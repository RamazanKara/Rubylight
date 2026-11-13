use std::ffi::c_void;

use windows::core::{s, Interface, PCSTR};
use windows::Win32::Graphics::Direct3D::{
    Fxc::*, ID3DBlob, D3D11_SRV_DIMENSION_TEXTURE2DARRAY, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, D3D_SRV_DIMENSION_TEXTURE2D,
};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::IDXGISwapChain;

use crate::media;
use super::gpu_planes::{Lease, Pin, Planes};
use super::pyrowave_sdk::fence_ready_within;

pub struct Filter {
    context: ID3D11DeviceContext,
    deferred: ID3D11DeviceContext,
    input: [Option<ID3D11ShaderResourceView>; 1],
    target: [Option<ID3D11RenderTargetView>; 1],
    vertex: ID3D11VertexShader,
    fragment: ID3D11PixelShader,
    constants: [Option<ID3D11Buffer>; 1],
    sampler: [Option<ID3D11SamplerState>; 1],
    size: (u32, u32),
}

impl Filter {
    pub unsafe fn new(input: *mut c_void, target: *mut c_void, source: &[u8]) -> windows::core::Result<Self> {
        let input = ID3D11Texture2D::from_raw_borrowed(&input).unwrap();
        let target = ID3D11Texture2D::from_raw_borrowed(&target).unwrap();
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        target.GetDesc(&mut desc);
        let device = target.GetDevice()?;
        let context = device.GetImmediateContext()?;
        let mut deferred = None;
        device.CreateDeferredContext(0, Some(&mut deferred))?;
        let mut view = None;
        device.CreateShaderResourceView(input, None, Some(&mut view))?;
        let mut target_view = None;
        device.CreateRenderTargetView(target, None, Some(&mut target_view))?;
        let code = compile(source, s!("vertex"), s!("vs_5_0"))?;
        let mut vertex = None;
        device.CreateVertexShader(std::slice::from_raw_parts(code.GetBufferPointer().cast(), code.GetBufferSize()), None, Some(&mut vertex))?;
        let code = compile(source, s!("fragment"), s!("ps_5_0"))?;
        let mut fragment = None;
        device.CreatePixelShader(std::slice::from_raw_parts(code.GetBufferPointer().cast(), code.GetBufferSize()), None, Some(&mut fragment))?;
        let mut constants = None;
        device.CreateBuffer(&D3D11_BUFFER_DESC { ByteWidth: 32, Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32, ..Default::default() }, None, Some(&mut constants))?;
        let mut sampler = None;
        device.CreateSamplerState(&D3D11_SAMPLER_DESC { Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP, AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressW: D3D11_TEXTURE_ADDRESS_CLAMP, MaxLOD: f32::MAX, ..Default::default() }, Some(&mut sampler))?;
        Ok(Self { context, deferred: deferred.unwrap(), input: [view], target: [target_view],
            vertex: vertex.unwrap(), fragment: fragment.unwrap(), constants: [constants], sampler: [sampler], size: (desc.Width, desc.Height) })
    }

    pub fn draw(&self, parameters: &[u32; 8], area: Option<(u32, u32)>) -> bool {
        unsafe {
            let size = area.unwrap_or(self.size);
            let c = &self.deferred;
            c.ClearState();
            c.UpdateSubresource(self.constants[0].as_ref().unwrap(), 0, None, parameters.as_ptr().cast(), 0, 0);
            c.OMSetRenderTargets(Some(&self.target), None);
            c.RSSetViewports(Some(&[D3D11_VIEWPORT { Width: size.0 as f32, Height: size.1 as f32,
                MinDepth: 0.0, MaxDepth: 1.0, ..Default::default() }]));
            c.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            c.VSSetShader(&self.vertex, None);
            c.PSSetShader(&self.fragment, None);
            c.PSSetShaderResources(0, Some(&self.input));
            c.PSSetSamplers(0, Some(&self.sampler));
            c.PSSetConstantBuffers(0, Some(&self.constants));
            c.Draw(3, 0);
            let mut commands = None;
            if c.FinishCommandList(false, Some(&mut commands)).is_err() { return false; }
            self.context.ExecuteCommandList(commands.as_ref().unwrap(), true);
            true
        }
    }
}

/// Converts the GPU PyroWave decoder's planes to linear RGB in the renderer's target texture.
pub struct PlanesPass {
    context: ID3D11DeviceContext,
    queue: ID3D11DeviceContext4,
    deferred: ID3D11DeviceContext,
    device: ID3D11Device,
    target: [Option<ID3D11RenderTargetView>; 1],
    vertex: ID3D11VertexShader,
    fragment: ID3D11PixelShader,
    constants: [Option<ID3D11Buffer>; 1],
    sampler: [Option<ID3D11SamplerState>; 1],
    /// Draws only inside the scissor rectangle, so a slice writes its rows alone.
    raster: ID3D11RasterizerState,
    size: (u32, u32),
    /// Shader resource views of the plane sets seen so far, for the `Planes` they belong to.
    views: std::collections::HashMap<usize, [Option<ID3D11ShaderResourceView>; 3]>,
    owner: Option<std::sync::Arc<Planes>>,
    /// The textures of software-decoded planar pictures (`draw_frame`).
    frame: Option<FramePlanes>,
    /// The copy of the last 4:4:4 decoder picture (`draw_surface`) and the entry that reads it,
    /// made with the first one.
    surface: Option<SurfaceCopy>,
    surface_fragment: Option<ID3D11PixelShader>,
    /// The shaders and constants of `draw_direct`, made with the first one.
    direct: Option<DirectShaders>,
    /// SDL's back buffer as a render target, for `draw_direct`.
    back_buffer: BackBuffer,
    /// The 4:4:4 surface `bind_surface` left, for `draw_direct` and `draw_bound_surface`.
    bound: Option<BoundSurface>,
    /// Views of the decoder's readable textures' slices, for the one texture they are of.
    surface_views: Option<(usize, SliceViews)>,
    /// Frame Warp's HUD mask, and its view for the next `draw_direct` (`t6`, `warp_row`).
    mask: super::frame_warp::D3d11Mask,
    mask_view: Option<ID3D11ShaderResourceView>,
}

/// A decoder texture's slice views by slice and format.
type SliceViews = std::collections::HashMap<(u32, i32), ID3D11ShaderResourceView>;

/// The pixel shaders of the direct draw (planes_direct.hlsl) and its constants (b1).
struct DirectShaders {
    planes: ID3D11PixelShader,
    surface: ID3D11PixelShader,
    surface_array: ID3D11PixelShader,
    /// `surfaceArray444`: an in-place surface converted into the RGBA64F texture.
    surface_array_target: ID3D11PixelShader,
    constants: ID3D11Buffer,
}

/// A 4:4:4 surface ready to be drawn: read in place through `view` (a slice of the decoder's
/// texture array), or copied into the pass's `SurfaceCopy`. `parameters` are the surface
/// entries' (`surface_parameters`).
struct BoundSurface {
    view: Option<ID3D11ShaderResourceView>,
    parameters: [u32; 8],
}

/// SDL's back buffer as a render target, made for the swap chain and output size it was made at.
/// It is let go before SDL resizes the buffers (a size change, or a frame that is not drawn
/// here): ResizeBuffers fails while a buffer is still referenced.
#[derive(Default)]
pub struct BackBuffer {
    view: Option<(usize, (i32, i32), ID3D11RenderTargetView)>,
}

impl BackBuffer {
    /// Lets go of the view when it is not for `swap_chain` at `size`. Call it before SDL runs its
    /// queue (`SDL_FlushRenderer`), which is where SDL resizes.
    pub fn keep_if(&mut self, swap_chain: usize, size: (i32, i32)) {
        if !self.view.as_ref().is_some_and(|(chain, at, _)| *chain == swap_chain && *at == size) {
            self.view = None;
        }
    }

    /// Lets go of the view, for a frame that does not draw with it.
    pub fn release(&mut self) {
        self.view = None;
    }

    /// The view of buffer 0 of `swap_chain` (made if there is none for it), for the `size` the
    /// output has.
    ///
    /// # Safety
    /// `swap_chain` is the live `IDXGISwapChain` of the renderer.
    pub unsafe fn view(&mut self, swap_chain: *mut c_void, size: (i32, i32)) -> Result<ID3D11RenderTargetView, String> {
        if self.view.is_none() {
            let chain = IDXGISwapChain::from_raw_borrowed(&swap_chain).ok_or("the renderer has no swap chain")?;
            let buffer = chain.GetBuffer::<ID3D11Texture2D>(0).map_err(|e| format!("the back buffer: {e}"))?;
            let device = buffer.GetDevice().map_err(|e| format!("the back buffer's device: {e}"))?;
            let mut target = None;
            device.CreateRenderTargetView(&buffer, None, Some(&mut target)).map_err(|e| format!("the back buffer's view: {e}"))?;
            self.view = Some((swap_chain as usize, size, target.ok_or("no back buffer view")?));
        }
        Ok(self.view.as_ref().expect("made above").2.clone())
    }

    #[cfg(test)]
    pub fn is_held(&self) -> bool {
        self.view.is_some()
    }

    #[cfg(test)]
    pub fn hold(&mut self, swap_chain: usize, size: (i32, i32), view: ID3D11RenderTargetView) {
        self.view = Some((swap_chain, size, view));
    }
}

/// What `PlanesPass::draw_direct` reads.
pub enum DirectSource<'a> {
    /// A GPU-decoded picture's plane set, once `fence_value` is signalled on the decode fence.
    Lease { lease: &'a Lease, fence_value: u64, deadline: std::time::Duration },
    /// The software-decoded planes `upload_frame` put into the pass's textures.
    Frame,
    /// The 4:4:4 surface `bind_surface` bound.
    Surface,
}

/// The source of the plane pass with the 4:4:4 surface entry (`surface444.hlsl`) appended.
fn surface_source() -> Vec<u8> {
    [include_bytes!("../pyrowave.hlsl").as_slice(), b"\n", include_bytes!("../surface444.hlsl")].concat()
}

/// The source of the direct draw: the plane shader with the surface entry and the direct entries
/// (`planes_direct.hlsl`) appended.
fn direct_source() -> Vec<u8> {
    [surface_source().as_slice(), b"\n", include_bytes!("../planes_direct.hlsl")].concat()
}

/// How the surface entry reads a 4:4:4 D3D11VA surface of `format` (`media::FRAME_D3D11_444`):
/// the view format of its copy and the entry's flags for it. AYUV is read as R8G8B8A8 (Cr, Cb, Y,
/// A); Y410 as R10G10B10A2 and Y416 as R16G16B16A16 (Cb, Y, Cr, A), both 10-bit (flag 16), Y416's
/// samples in the high bits (flag 256). These are the views Direct3D allows on these formats.
/// None for any other format.
pub(crate) fn surface_view(format: DXGI_FORMAT) -> Option<(DXGI_FORMAT, u32)> {
    match format {
        DXGI_FORMAT_AYUV => Some((DXGI_FORMAT_R8G8B8A8_UNORM, 0)),
        DXGI_FORMAT_Y410 => Some((DXGI_FORMAT_R10G10B10A2_UNORM, 16)),
        DXGI_FORMAT_Y416 => Some((DXGI_FORMAT_R16G16B16A16_UNORM, 16 | 256)),
        _ => None,
    }
}

/// The surface entry's constants for a surface of `format`: the planar picture's
/// (`present::planar_parameters`: size, range, matrix, HDR, HLG) with the depth and sample flags of
/// the surface in place of the planes' (16, 64).
pub(crate) fn surface_parameters(mut parameters: [u32; 8], format: DXGI_FORMAT) -> Option<[u32; 8]> {
    let (_, flags) = surface_view(format)?;
    parameters[6] = parameters[6] & !(16 | 64 | 256) | flags;
    Some(parameters)
}

/// A texture of the pass that a 4:4:4 decoder picture is copied into for the surface entry: the
/// decoder's textures are seldom shader-readable (`direct::bind_decoder_textures`).
struct SurfaceCopy {
    format: DXGI_FORMAT,
    size: (u32, u32),
    texture: ID3D11Texture2D,
    view: [Option<ID3D11ShaderResourceView>; 1],
}

impl PlanesPass {
    /// `target` is the `ID3D11Texture2D` of the renderer's RGBA64F picture texture.
    pub unsafe fn new(target: *mut c_void) -> windows::core::Result<Self> {
        let target = ID3D11Texture2D::from_raw_borrowed(&target).ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_POINTER))?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        target.GetDesc(&mut desc);
        let device = target.GetDevice()?;
        let context = device.GetImmediateContext()?;
        let queue = context.cast::<ID3D11DeviceContext4>()?;
        let mut deferred = None;
        device.CreateDeferredContext(0, Some(&mut deferred))?;
        let mut target_view = None;
        device.CreateRenderTargetView(target, None, Some(&mut target_view))?;
        let source = include_bytes!("../pyrowave.hlsl");
        let code = compile(source, s!("vertex"), s!("vs_5_0"))?;
        let mut vertex = None;
        device.CreateVertexShader(std::slice::from_raw_parts(code.GetBufferPointer().cast(), code.GetBufferSize()), None, Some(&mut vertex))?;
        let code = compile(source, s!("planes"), s!("ps_5_0"))?;
        let mut fragment = None;
        device.CreatePixelShader(std::slice::from_raw_parts(code.GetBufferPointer().cast(), code.GetBufferSize()), None, Some(&mut fragment))?;
        let mut constants = None;
        device.CreateBuffer(&D3D11_BUFFER_DESC { ByteWidth: 32, Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32, ..Default::default() }, None, Some(&mut constants))?;
        let mut sampler = None;
        device.CreateSamplerState(&D3D11_SAMPLER_DESC { Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
            AddressU: D3D11_TEXTURE_ADDRESS_CLAMP, AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressW: D3D11_TEXTURE_ADDRESS_CLAMP, MaxLOD: f32::MAX, ..Default::default() }, Some(&mut sampler))?;
        let mut raster = None;
        device.CreateRasterizerState(&D3D11_RASTERIZER_DESC { FillMode: D3D11_FILL_SOLID, CullMode: D3D11_CULL_NONE,
            DepthClipEnable: true.into(), ScissorEnable: true.into(), ..Default::default() }, Some(&mut raster))?;
        Ok(Self { context, queue, deferred: deferred.unwrap(), device, target: [target_view],
            vertex: vertex.unwrap(), fragment: fragment.unwrap(), constants: [constants], sampler: [sampler],
            raster: raster.unwrap(), size: (desc.Width, desc.Height), views: Default::default(), owner: None, frame: None,
            surface: None, surface_fragment: None, direct: None, back_buffer: BackBuffer::default(), bound: None, surface_views: None,
            mask: Default::default(), mask_view: None })
    }

    /// Frame Warp's row of `direct_planes::Constants` for a `draw_direct` shifted by `warp`, with
    /// its HUD mask made ready for that draw.
    pub fn warp_row(&mut self, warp: &super::frame_warp::WarpDraw) -> [f32; 4] {
        // SAFETY: The pass's device, used on the presenting thread.
        self.mask_view = unsafe { self.mask.view(&self.device, warp) };
        warp.row(self.mask_view.is_some())
    }

    /// Compiles the plane shader for a probe: whether the pass can be built at all.
    pub fn compiles() -> Result<(), String> {
        let source = include_bytes!("../pyrowave.hlsl");
        // SAFETY: Plain compiler calls on static source.
        unsafe {
            compile(source, s!("vertex"), s!("vs_5_0")).map_err(|e| format!("the PyroWave plane shader does not compile: {e}"))?;
            compile(source, s!("planes"), s!("ps_5_0")).map_err(|e| format!("the PyroWave plane shader does not compile: {e}"))?;
            compile(&surface_source(), s!("surface444"), s!("ps_5_0")).map_err(|e| format!("the 4:4:4 picture shader does not compile: {e}"))?;
            for entry in [s!("planesDirect"), s!("surfaceDirect"), s!("surfaceArrayDirect"), s!("surfaceArray444")] {
                compile(&direct_source(), entry, s!("ps_5_0")).map_err(|e| format!("the direct plane shader does not compile: {e}"))?;
            }
        }
        Ok(())
    }

    /// The direct draw's shaders and constants, made on the first use.
    fn direct_shaders(&mut self) -> Result<&DirectShaders, String> {
        if self.direct.is_none() {
            let source = direct_source();
            let pixel = |entry: PCSTR| -> Result<ID3D11PixelShader, String> {
                // SAFETY: A compiler call on static source and a shader made on the pass's device.
                unsafe {
                    let code = compile(&source, entry, s!("ps_5_0")).map_err(|e| format!("the direct plane shader does not compile: {e}"))?;
                    let mut shader = None;
                    self.device.CreatePixelShader(std::slice::from_raw_parts(code.GetBufferPointer().cast(), code.GetBufferSize()), None, Some(&mut shader))
                        .map_err(|e| format!("the direct plane shader cannot be made: {e}"))?;
                    shader.ok_or_else(|| "no direct plane shader".to_string())
                }
            };
            let (planes, surface, surface_array, surface_array_target) =
                (pixel(s!("planesDirect"))?, pixel(s!("surfaceDirect"))?, pixel(s!("surfaceArrayDirect"))?, pixel(s!("surfaceArray444"))?);
            let mut constants = None;
            // SAFETY: A buffer on the pass's device.
            unsafe {
                self.device.CreateBuffer(&D3D11_BUFFER_DESC { ByteWidth: std::mem::size_of::<super::direct_planes::Constants>() as u32, Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32, ..Default::default() }, None, Some(&mut constants))
                    .map_err(|e| format!("the direct plane constants cannot be made: {e}"))?;
            }
            self.direct = Some(DirectShaders { planes, surface, surface_array, surface_array_target, constants: constants.ok_or("no direct plane constants")? });
        }
        Ok(self.direct.as_ref().expect("made above"))
    }

    /// Lets go of SDL's back buffer, for a frame the pass does not draw into it (SDL may resize then).
    pub fn release_back_buffer(&mut self) {
        self.back_buffer.release();
    }

    /// The view of SDL's back buffer to draw into, once SDL's queue has run (the caller flushed
    /// the renderer); see `BackBuffer`.
    ///
    /// # Safety
    /// `swap_chain` is the live swap chain of the renderer this pass's device belongs to.
    pub unsafe fn back_buffer(&mut self, swap_chain: *mut c_void, size: (i32, i32)) -> Result<ID3D11RenderTargetView, String> {
        self.back_buffer.view(swap_chain, size)
    }

    /// Call before SDL runs its queue: a view for another swap chain or size goes.
    pub fn keep_back_buffer_if(&mut self, swap_chain: usize, size: (i32, i32)) {
        self.back_buffer.keep_if(swap_chain, size);
    }

    /// Draws a 4:4:4 D3D11VA picture (`media::FRAME_D3D11_444`: AYUV, Y410 or Y416) into the
    /// target, converted as a planar picture is: its slice is copied under FFmpeg's device lock
    /// into a texture of the pass, which the surface entry reads.
    pub fn draw_surface(&mut self, frame: &media::Frame, info: &media::FrameInfo, parameters: &[u32; 8]) -> Result<(), String> {
        let surface = frame.d3d11_surface().ok_or("the picture is not a Direct3D 11 decoder surface")?;
        let raw = surface.texture;
        // SAFETY: The live frame's texture array, borrowed while the frame is.
        let source = unsafe { ID3D11Texture2D::from_raw_borrowed(&raw) }.ok_or("the picture has no texture")?;
        let (width, height) = (info.width.max(0) as u32, info.height.max(0) as u32);
        self.draw_texture(source, surface.slice, (width, height), parameters, &mut |copy: &mut dyn FnMut()| frame.with_decoder_lock(copy))
    }

    /// `draw_surface` for slice `slice` of `source`, whose `size` top-left pixels are the picture.
    /// `lock` runs the copy under the lock of the device's other user (FFmpeg's decoder).
    fn draw_texture(&mut self, source: &ID3D11Texture2D, slice: u32, size: (u32, u32), parameters: &[u32; 8],
        lock: &mut dyn FnMut(&mut dyn FnMut())) -> Result<(), String> {
        let parameters = self.copy_texture(source, slice, size, parameters, lock)?;
        self.draw_copied_surface(&parameters)
    }

    /// Draws the surface `copy_texture` copied last into the target.
    fn draw_copied_surface(&mut self, parameters: &[u32; 8]) -> Result<(), String> {
        let fragment = self.surface_fragment()?;
        let view = self.surface.as_ref().ok_or("no 4:4:4 picture was copied")?.view.clone();
        self.record_pass(&fragment, 4, &view, parameters, None).map_err(|e| format!("cannot record the 4:4:4 picture pass: {e}"))
    }

    /// Checks slice `slice` of `source` (its `size` top-left pixels are the picture) against what
    /// the pass reads and copies it into the pass's texture; the constants for its entries.
    fn copy_texture(&mut self, source: &ID3D11Texture2D, slice: u32, size: (u32, u32), parameters: &[u32; 8],
        lock: &mut dyn FnMut(&mut dyn FnMut())) -> Result<[u32; 8], String> {
        let (desc, parameters) = self.check_surface(source, slice, size, parameters)?;
        let (view_format, _) = surface_view(desc.Format).expect("a format check_surface accepted");
        if self.surface.as_ref().is_none_or(|copy| copy.format != desc.Format || copy.size != size) {
            self.surface = None;
            // SAFETY: A texture and view on the pass's own device.
            self.surface = Some(unsafe { SurfaceCopy::new(&self.device, desc.Format, view_format, size) }
                .map_err(|e| format!("cannot make the {}x{} copy of the decoder's 4:4:4 pictures: {e}", size.0, size.1))?);
        }
        let copy = self.surface.as_ref().unwrap();
        let region = D3D11_BOX { left: 0, top: 0, front: 0, right: size.0, bottom: size.1, back: 1 };
        // Subresource `slice` is the picture: the decoder's textures have one mip level.
        let subresource = slice * desc.MipLevels.max(1);
        // SAFETY: Both textures are live on this device, the region is inside both, and the
        // immediate context is used under the lock FFmpeg holds while it decodes.
        lock(&mut || unsafe { self.context.CopySubresourceRegion(&copy.texture, 0, 0, 0, 0, source, subresource, Some(&region)) });
        Ok(parameters)
    }

    /// What `draw_texture` checks before it reads slice `slice` of `source`: a format this pass
    /// reads, a picture that fits, the pass's own device. The texture's description and the
    /// constants of the surface entries.
    fn check_surface(&self, source: &ID3D11Texture2D, slice: u32, size: (u32, u32), parameters: &[u32; 8]) -> Result<(D3D11_TEXTURE2D_DESC, [u32; 8]), String> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: A live texture.
        unsafe { source.GetDesc(&mut desc) };
        let Some(parameters) = surface_parameters(*parameters, desc.Format) else {
            return Err(format!("the decoder's pictures are DXGI format {}, not a 4:4:4 format this pass reads", desc.Format.0));
        };
        if size.0 == 0 || size.1 == 0 || size.0 > desc.Width || size.1 > desc.Height || slice >= desc.ArraySize {
            return Err(format!("a {}x{} picture in slice {slice} does not fit the decoder's {}x{}x{} textures", size.0, size.1, desc.Width, desc.Height, desc.ArraySize));
        }
        // SAFETY: A live texture's device.
        let device = unsafe { source.GetDevice() }.map_err(|e| format!("the decoder's texture has no device: {e}"))?;
        if device.as_raw() != self.device.as_raw() {
            return Err("the decoder's pictures are on another Direct3D 11 device than the renderer's".into());
        }
        Ok((desc, parameters))
    }

    /// Readies a 4:4:4 D3D11VA picture for `draw_direct` (`DirectSource::Surface`) and
    /// `draw_bound_surface`. A readable decoder texture is read in place (the caller holds the
    /// frame and draws under its lock): true. Otherwise, or when no view of it can be made, its
    /// slice is copied now into the pass's texture (under the decoder's lock): false.
    pub fn bind_surface(&mut self, frame: &media::Frame, info: &media::FrameInfo, parameters: &[u32; 8], in_place: bool) -> Result<bool, String> {
        let surface = frame.d3d11_surface().ok_or("the picture is not a Direct3D 11 decoder surface")?;
        let raw = surface.texture;
        // SAFETY: The live frame's texture array, borrowed while the frame is.
        let source = unsafe { ID3D11Texture2D::from_raw_borrowed(&raw) }.ok_or("the picture has no texture")?;
        let size = (info.width.max(0) as u32, info.height.max(0) as u32);
        if in_place && surface.readable {
            let (desc, parameters) = self.check_surface(source, surface.slice, size, parameters)?;
            match self.slice_view(source, raw as usize, surface.slice, desc.Format) {
                Ok(view) => {
                    self.bound = Some(BoundSurface { view: Some(view), parameters });
                    return Ok(true);
                }
                Err(error) => eprintln!("nightjar: the decoder's 4:4:4 pictures cannot be read in place ({error}); they are copied"),
            }
        }
        let parameters = self.copy_texture(source, surface.slice, size, parameters, &mut |copy: &mut dyn FnMut()| frame.with_decoder_lock(copy))?;
        self.bound = Some(BoundSurface { view: None, parameters });
        Ok(false)
    }

    /// A view of slice `slice` of the decoder's texture `source` as the surface entries read it.
    fn slice_view(&mut self, source: &ID3D11Texture2D, key: usize, slice: u32, format: DXGI_FORMAT) -> Result<ID3D11ShaderResourceView, String> {
        let (view_format, _) = surface_view(format).ok_or("not a 4:4:4 format")?;
        if !self.surface_views.as_ref().is_some_and(|(texture, _)| *texture == key) {
            self.surface_views = Some((key, Default::default()));
        }
        let (_, views) = self.surface_views.as_mut().unwrap();
        if let Some(view) = views.get(&(slice, format.0)) {
            return Ok(view.clone());
        }
        let mut view = None;
        // SAFETY: A view of a live texture made with the shader resource bind flag.
        unsafe {
            self.device.CreateShaderResourceView(source, Some(&D3D11_SHADER_RESOURCE_VIEW_DESC { Format: view_format, ViewDimension: D3D11_SRV_DIMENSION_TEXTURE2DARRAY,
                Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 { Texture2DArray: D3D11_TEX2D_ARRAY_SRV { MostDetailedMip: 0, MipLevels: 1, FirstArraySlice: slice, ArraySize: 1 } } }), Some(&mut view))
                .map_err(|e| format!("a view of slice {slice}: {e}"))?;
        }
        let view = view.ok_or("no view")?;
        views.insert((slice, format.0), view.clone());
        Ok(view)
    }

    /// Draws the bound surface into the target (the RGBA64F texture), for what reads the texture.
    /// `lock` runs the submission under the decoder's lock for a surface read in place.
    pub fn draw_bound_surface(&mut self, lock: &mut dyn FnMut(&mut dyn FnMut())) -> Result<(), String> {
        let bound = self.bound.as_ref().ok_or("no 4:4:4 picture is bound")?;
        let (view, parameters) = (bound.view.clone(), bound.parameters);
        match view {
            None => self.draw_copied_surface(&parameters),
            Some(view) => {
                let fragment = self.direct_shaders()?.surface_array_target.clone();
                self.record_pass_locked(&fragment, 5, &[Some(view)], &parameters, None, lock).map_err(|e| format!("cannot record the 4:4:4 picture pass: {e}"))
            }
        }
    }

    /// The surface entry's pixel shader, compiled with the first 4:4:4 picture.
    fn surface_fragment(&mut self) -> Result<ID3D11PixelShader, String> {
        if let Some(fragment) = &self.surface_fragment {
            return Ok(fragment.clone());
        }
        // SAFETY: A compiler call on static source and a shader made on the pass's device.
        let fragment = unsafe {
            let code = compile(&surface_source(), s!("surface444"), s!("ps_5_0")).map_err(|e| format!("the 4:4:4 picture shader does not compile: {e}"))?;
            let mut fragment = None;
            self.device.CreatePixelShader(std::slice::from_raw_parts(code.GetBufferPointer().cast(), code.GetBufferSize()), None, Some(&mut fragment))
                .map_err(|e| format!("the 4:4:4 picture shader cannot be made: {e}"))?;
            fragment.unwrap()
        };
        self.surface_fragment = Some(fragment.clone());
        Ok(fragment)
    }

    /// Points the pass at the plane sets of `planes`, letting go of the views and the references
    /// to the previous decoder's sets (a new setup makes a new decoder with new textures).
    pub fn retarget(&mut self, planes: &std::sync::Arc<Planes>) {
        if !self.owner.as_ref().is_some_and(|owner| std::sync::Arc::ptr_eq(owner, planes)) {
            self.views.clear();
            self.owner = Some(planes.clone());
        }
    }

    fn views(&mut self, planes: &std::sync::Arc<Planes>, set: usize) -> windows::core::Result<[Option<ID3D11ShaderResourceView>; 3]> {
        self.retarget(planes);
        if let Some(views) = self.views.get(&set) { return Ok(views.clone()); }
        let mut views = [None, None, None];
        for (view, texture) in views.iter_mut().zip(planes.textures(set)) {
            // SAFETY: A default view of a live texture that has the shader resource bind flag.
            unsafe { self.device.CreateShaderResourceView(texture, None, Some(view))? };
        }
        self.views.insert(set, views.clone());
        Ok(views)
    }

    /// Draws the picture in `lease` into the target once the decode has finished (all rows, or
    /// `rows` of them), then signals the reuse fence behind the draw and hands the plane set
    /// back. `deadline` bounds the wait for the decode fence before the (unbounded) GPU `Wait`
    /// is queued; an error is the loud failure, and the stream ends with it.
    pub fn draw(&mut self, lease: &Lease, fence_value: u64, parameters: &[u32; 8], deadline: std::time::Duration, rows: Option<(u32, u32)>) -> Result<(), String> {
        let value = lease.planes().next_reuse_value();
        let planes = lease.planes().clone();
        self.record(&planes, lease.set(), fence_value, parameters, deadline, rows)?;
        // The next decode into this set waits for the draw that read it. The signal is flushed
        // now: a decode that waits on a signal still in the queue would wait for ever.
        hand_back(lease, value, || self.signal(&planes, value))
    }

    /// Draws `rows` of a slice that is exact in the plane set once `fence_value` is signalled.
    /// The set stays with its frame; the next decode into it waits for this draw.
    pub fn draw_slice(&mut self, pin: &Pin, fence_value: u64, parameters: &[u32; 8], deadline: std::time::Duration, rows: (u32, u32)) -> Result<(), String> {
        let value = pin.planes().next_reuse_value();
        // Noted before the draw is queued, so a decode that starts while it is recorded waits for it.
        pin.read_until(value);
        let planes = pin.planes().clone();
        let recorded = self.record(&planes, pin.set(), fence_value, parameters, deadline, Some(rows));
        // Signalled also when the draw was not recorded: the value is noted, and a decode into the
        // set waits for it.
        after_slice_draw(recorded, || self.signal(&planes, value))
    }

    fn signal(&self, planes: &Planes, value: u64) -> Result<(), String> {
        // SAFETY: The immediate context belongs to this (the presenting) thread.
        unsafe {
            self.queue.Signal(planes.reuse_fence(), value).map_err(|e| format!("cannot release the PyroWave planes: {e}"))?;
            self.context.Flush();
        }
        Ok(())
    }

    fn record(&mut self, planes: &std::sync::Arc<Planes>, set: usize, fence_value: u64, parameters: &[u32; 8], deadline: std::time::Duration, rows: Option<(u32, u32)>) -> Result<(), String> {
        self.retarget(planes);
        fence_ready_within(planes.decode_fence(), fence_value, deadline)?;
        let views = self.views(planes, set).map_err(|e| format!("cannot read the PyroWave planes: {e}"))?;
        // SAFETY: The immediate context belongs to this (the presenting) thread.
        unsafe {
            self.queue.Wait(planes.decode_fence(), fence_value).map_err(|e| format!("cannot order the picture after the PyroWave decode: {e}"))?;
        }
        self.record_views(&views, parameters, rows).map_err(|e| format!("cannot record the PyroWave plane pass: {e}"))
    }

    /// Draws a software-decoded planar picture (FFmpeg's yuv444p, yuv420p10 or yuv444p10) into
    /// the target: its planes go into textures of the pass and are converted like the decoder's.
    pub fn draw_frame(&mut self, info: &media::FrameInfo, parameters: &[u32; 8]) -> Result<(), String> {
        self.make_frame(info)?;
        let frame = self.frame.as_ref().unwrap();
        for (plane, texture) in frame.textures.iter().enumerate() {
            // SAFETY: The plane pointer and pitch come from the live AVFrame, whose plane holds
            // the texture's rows; the deferred context copies the data when it records.
            unsafe { self.deferred.UpdateSubresource(texture, 0, None, info.data[plane].cast_const().cast(), info.linesize[plane] as u32, 0) };
        }
        let views = frame.views.clone();
        self.record_views(&views, parameters, None).map_err(|e| format!("cannot record the picture's plane pass: {e}"))
    }

    /// Records and runs the plane pass over `views` (bound at t1 to t3), after anything already
    /// recorded in the deferred context.
    fn record_views(&self, views: &[Option<ID3D11ShaderResourceView>; 3], parameters: &[u32; 8], rows: Option<(u32, u32)>) -> windows::core::Result<()> {
        self.record_pass(&self.fragment, 1, views, parameters, rows)
    }

    /// Records and runs `fragment` over `views` (bound from t`slot` on): the plane entry, or the
    /// surface entry over one 4:4:4 picture (t4).
    fn record_pass(&self, fragment: &ID3D11PixelShader, slot: u32, views: &[Option<ID3D11ShaderResourceView>], parameters: &[u32; 8], rows: Option<(u32, u32)>) -> windows::core::Result<()> {
        self.record_pass_locked(fragment, slot, views, parameters, rows, &mut |submit| submit())
    }

    /// `record_pass`, the submission run by `lock` (under the decoder's lock for a decoder
    /// texture read in place).
    fn record_pass_locked(&self, fragment: &ID3D11PixelShader, slot: u32, views: &[Option<ID3D11ShaderResourceView>], parameters: &[u32; 8], rows: Option<(u32, u32)>,
        lock: &mut dyn FnMut(&mut dyn FnMut())) -> windows::core::Result<()> {
        // SAFETY: The immediate context belongs to this (the presenting) thread; the deferred
        // context records the draw so SDL's cached immediate-context state is restored.
        unsafe {
            let c = &self.deferred;
            c.ClearState();
            c.UpdateSubresource(self.constants[0].as_ref().unwrap(), 0, None, parameters.as_ptr().cast(), 0, 0);
            c.OMSetRenderTargets(Some(&self.target), None);
            // A whole picture draws with the default state `ClearState` left (no scissor); only a
            // slice scissors to its rows.
            if let Some((first, end)) = rows {
                let (top, bottom) = (first.min(self.size.1), end.min(self.size.1));
                c.RSSetState(&self.raster);
                c.RSSetScissorRects(Some(&[windows::Win32::Foundation::RECT { left: 0, top: top as i32, right: self.size.0 as i32, bottom: bottom as i32 }]));
            }
            c.RSSetViewports(Some(&[D3D11_VIEWPORT { Width: self.size.0 as f32, Height: self.size.1 as f32,
                MinDepth: 0.0, MaxDepth: 1.0, ..Default::default() }]));
            c.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            c.VSSetShader(&self.vertex, None);
            c.PSSetShader(fragment, None);
            c.PSSetShaderResources(slot, Some(views));
            c.PSSetSamplers(0, Some(&self.sampler));
            c.PSSetConstantBuffers(0, Some(&self.constants));
            c.Draw(3, 0);
            let mut commands = None;
            c.FinishCommandList(false, Some(&mut commands))?;
            let commands = commands.unwrap();
            lock(&mut || self.context.ExecuteCommandList(&commands, true));
        }
        Ok(())
    }

    /// Draws the picture `source` names straight into `target` (SDL's back buffer, a view of it
    /// from `back_buffer`) at `viewport` (x, y, width, height in pixels), converted and scaled in
    /// the one draw (`planes_direct.hlsl`). `parameters` are the plane pass's constants for the
    /// picture and `constants` what SDL's draw of the texture would have applied. A plane set
    /// stays with its lease, which the caller holds and gives back with `give_back` after the
    /// last draw. `lock` runs the submission (under the decoder's lock for a surface read in place).
    pub fn draw_direct(&mut self, target: &ID3D11RenderTargetView, viewport: [f32; 4], source: &DirectSource, parameters: &[u32; 8],
        constants: &super::direct_planes::Constants, lock: &mut dyn FnMut(&mut dyn FnMut())) -> Result<(), String> {
        let (planes_entry, surface_entry, array_entry, buffer) = {
            let shaders = self.direct_shaders()?;
            (shaders.planes.clone(), shaders.surface.clone(), shaders.surface_array.clone(), shaders.constants.clone())
        };
        let (fragment, slot, views, parameters): (ID3D11PixelShader, u32, Vec<Option<ID3D11ShaderResourceView>>, [u32; 8]) = match source {
            DirectSource::Lease { lease, fence_value, deadline } => {
                let planes = lease.planes().clone();
                self.retarget(&planes);
                fence_ready_within(planes.decode_fence(), *fence_value, *deadline)?;
                let views = self.views(&planes, lease.set()).map_err(|e| format!("cannot read the PyroWave planes: {e}"))?;
                // SAFETY: The immediate context belongs to this (the presenting) thread.
                unsafe { self.queue.Wait(planes.decode_fence(), *fence_value) }.map_err(|e| format!("cannot order the picture after the PyroWave decode: {e}"))?;
                (planes_entry, 1, views.to_vec(), *parameters)
            }
            DirectSource::Frame => {
                let frame = self.frame.as_ref().ok_or("no planar picture was uploaded")?;
                (planes_entry, 1, frame.views.to_vec(), *parameters)
            }
            DirectSource::Surface => {
                let bound = self.bound.as_ref().ok_or("no 4:4:4 picture is bound")?;
                match &bound.view {
                    Some(view) => (array_entry, 5, vec![Some(view.clone())], bound.parameters),
                    None => (surface_entry, 4, self.surface.as_ref().ok_or("no 4:4:4 picture was copied")?.view.to_vec(), bound.parameters),
                }
            }
        };
        // SAFETY: The immediate context belongs to this (the presenting) thread; the deferred
        // context records the draw so SDL's cached immediate-context state is restored.
        unsafe {
            let c = &self.deferred;
            c.ClearState();
            c.UpdateSubresource(self.constants[0].as_ref().unwrap(), 0, None, parameters.as_ptr().cast(), 0, 0);
            c.UpdateSubresource(&buffer, 0, None, std::ptr::from_ref(constants).cast(), 0, 0);
            c.OMSetRenderTargets(Some(&[Some(target.clone())]), None);
            c.RSSetViewports(Some(&[D3D11_VIEWPORT { TopLeftX: viewport[0], TopLeftY: viewport[1], Width: viewport[2], Height: viewport[3], MinDepth: 0.0, MaxDepth: 1.0 }]));
            c.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            c.VSSetShader(&self.vertex, None);
            c.PSSetShader(&fragment, None);
            c.PSSetShaderResources(slot, Some(&views));
            // The HUD mask `warp_row` readied, when the constants say there is one.
            if constants.warp[2] > 0.0 && self.mask_view.is_some() {
                c.PSSetShaderResources(6, Some(std::slice::from_ref(&self.mask_view)));
            }
            c.PSSetSamplers(0, Some(&self.sampler));
            c.PSSetConstantBuffers(0, Some(&[self.constants[0].clone(), Some(buffer.clone())]));
            c.Draw(3, 0);
            let mut commands = None;
            c.FinishCommandList(false, Some(&mut commands)).map_err(|e| format!("cannot record the direct plane pass: {e}"))?;
            let commands = commands.ok_or("no command list")?;
            lock(&mut || self.context.ExecuteCommandList(&commands, true));
        }
        Ok(())
    }

    /// Draws a plane set a presenter holds into the target texture, as `draw` does but leaving
    /// the set with its lease (`give_back`).
    pub fn draw_held(&mut self, lease: &Lease, fence_value: u64, parameters: &[u32; 8], deadline: std::time::Duration) -> Result<(), String> {
        let planes = lease.planes().clone();
        self.record(&planes, lease.set(), fence_value, parameters, deadline, None)
    }

    /// Gives a plane set a presenter held back to the decoder, behind every draw of it queued so far.
    pub fn give_back(&self, lease: &Lease) -> Result<(), String> {
        let value = lease.planes().next_reuse_value();
        let planes = lease.planes().clone();
        hand_back(lease, value, || self.signal(&planes, value))
    }

    /// Puts a software-decoded planar picture's planes into the pass's textures, for
    /// `DirectSource::Frame` and `draw_uploaded_frame`.
    pub fn upload_frame(&mut self, info: &media::FrameInfo) -> Result<(), String> {
        self.make_frame(info)?;
        let frame = self.frame.as_ref().unwrap();
        for (plane, texture) in frame.textures.iter().enumerate() {
            // SAFETY: The plane pointer and pitch come from the live AVFrame, whose plane holds
            // the texture's rows; the deferred context copies the data when it records.
            unsafe { self.deferred.UpdateSubresource(texture, 0, None, info.data[plane].cast_const().cast(), info.linesize[plane] as u32, 0) };
        }
        // SAFETY: The deferred context records the uploads; the immediate one runs them.
        unsafe {
            let mut commands = None;
            self.deferred.FinishCommandList(false, Some(&mut commands)).map_err(|e| format!("cannot record the picture's plane upload: {e}"))?;
            self.context.ExecuteCommandList(commands.as_ref().ok_or("no command list")?, true);
        }
        Ok(())
    }

    /// Draws the planes `upload_frame` put in last into the target texture.
    pub fn draw_uploaded_frame(&mut self, parameters: &[u32; 8]) -> Result<(), String> {
        let views = self.frame.as_ref().ok_or("no planar picture was uploaded")?.views.clone();
        self.record_views(&views, parameters, None).map_err(|e| format!("cannot record the picture's plane pass: {e}"))
    }

    /// The textures for pictures of `info`'s shape, after checking that its planes are there.
    fn make_frame(&mut self, info: &media::FrameInfo) -> Result<(), String> {
        let layout = media::PlanarLayout::of(info).ok_or_else(|| format!("frame kind {} has no planes to draw", info.kind))?;
        // Checked before anything is recorded, so a refused picture leaves no upload behind.
        if let Some(plane) = (0..3).find(|&plane| info.data[plane].is_null() || info.linesize[plane] <= 0) {
            return Err(format!("plane {plane} of the picture is missing"));
        }
        if self.frame.as_ref().is_none_or(|frame| frame.layout != layout) {
            self.frame = None;
            // SAFETY: Textures and views on the pass's own device.
            self.frame = Some(unsafe { FramePlanes::new(&self.device, layout) }.map_err(|e| format!("cannot make the {}x{} picture planes: {e}", layout.width, layout.height))?);
        }
        Ok(())
    }
}

/// The textures a planar picture is uploaded into, kept while pictures keep their shape.
struct FramePlanes {
    layout: media::PlanarLayout,
    textures: [ID3D11Texture2D; 3],
    views: [Option<ID3D11ShaderResourceView>; 3],
}

impl FramePlanes {
    unsafe fn new(device: &ID3D11Device, layout: media::PlanarLayout) -> windows::core::Result<Self> {
        let format = if layout.ten_bit { DXGI_FORMAT_R16_UNORM } else { DXGI_FORMAT_R8_UNORM };
        let mut textures = Vec::with_capacity(3);
        let mut views = [None, None, None];
        for (plane, view) in views.iter_mut().enumerate() {
            let (width, height) = layout.plane_size(plane);
            let mut texture = None;
            device.CreateTexture2D(&D3D11_TEXTURE2D_DESC { Width: width, Height: height, MipLevels: 1, ArraySize: 1, Format: format,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32, ..Default::default() }, None, Some(&mut texture))?;
            let texture = texture.unwrap();
            device.CreateShaderResourceView(&texture, None, Some(view))?;
            textures.push(texture);
        }
        let textures: [ID3D11Texture2D; 3] = textures.try_into().unwrap_or_else(|_| unreachable!());
        Ok(Self { layout, textures, views })
    }
}

impl SurfaceCopy {
    unsafe fn new(device: &ID3D11Device, format: DXGI_FORMAT, view_format: DXGI_FORMAT, size: (u32, u32)) -> windows::core::Result<Self> {
        let mut texture = None;
        device.CreateTexture2D(&D3D11_TEXTURE2D_DESC { Width: size.0, Height: size.1, MipLevels: 1, ArraySize: 1, Format: format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32, ..Default::default() }, None, Some(&mut texture))?;
        let texture = texture.unwrap();
        let mut view = None;
        device.CreateShaderResourceView(&texture, Some(&D3D11_SHADER_RESOURCE_VIEW_DESC { Format: view_format, ViewDimension: D3D_SRV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_SRV { MostDetailedMip: 0, MipLevels: 1 } } }), Some(&mut view))?;
        Ok(Self { format, size, texture, view: [view] })
    }
}

/// Gives the plane set back as `value` on the reuse fence. The set goes back also when the signal
/// fails: the draw is queued and the error ends the stream, so the set must not stay held.
pub(crate) fn hand_back(lease: &Lease, value: u64, signal: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    let signalled = signal();
    lease.release_after(value);
    signalled
}

/// The end of a slice's draw: the reuse fence is signalled whether or not the draw was recorded,
/// and the error of the draw, if any, is the one reported.
pub(crate) fn after_slice_draw(recorded: Result<(), String>, signal: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    let signalled = signal();
    recorded.and(signalled)
}

/// `packed444.hlsl`'s `Presentation` block: the destination, the output's colour, then Frame
/// Warp's two rows.
const PRESENTATION_FLOATS: usize = 16;
const PRESENTATION_BYTES: u32 = PRESENTATION_FLOATS as u32 * 4;

pub struct Packed {
    context: ID3D11DeviceContext,
    deferred: ID3D11DeviceContext,
    input: ID3D11Texture2D,
    planes: [Option<ID3D11ShaderResourceView>; 2],
    target: [Option<ID3D11RenderTargetView>; 1],
    vertex: ID3D11VertexShader,
    fragment: ID3D11PixelShader,
    constants: [Option<ID3D11Buffer>; 1],
    presentation: [Option<ID3D11Buffer>; 1],
    width: i32,
    height: i32,
    /// SDL's back buffer as a render target, kept across frames (see `BackBuffer`).
    back_buffer: std::cell::RefCell<BackBuffer>,
    device: ID3D11Device,
    /// Frame Warp's HUD mask for `present` (`t2`).
    mask: std::cell::RefCell<super::frame_warp::D3d11Mask>,
}

/// Compiles `source`, with Frame Warp's shared HLSL spliced in where it includes it
/// (`frame_warp::hlsl`).
pub(crate) unsafe fn compile(source: &[u8], entry: PCSTR, profile: PCSTR) -> windows::core::Result<ID3DBlob> {
    let source = super::frame_warp::hlsl(source);
    let mut code = None;
    let mut errors: Option<ID3DBlob> = None;
    let result = D3DCompile(
        source.as_ptr().cast(),
        source.len(),
        PCSTR::null(),
        None,
        None,
        entry,
        profile,
        D3DCOMPILE_OPTIMIZATION_LEVEL3,
        0,
        &mut code,
        Some(&mut errors),
    );
    if result.is_err() {
        if let Some(errors) = errors {
            eprintln!("nightjar shader: {}", String::from_utf8_lossy(std::slice::from_raw_parts(errors.GetBufferPointer().cast(), errors.GetBufferSize())));
        }
    }
    result?;
    Ok(code.unwrap())
}

impl Packed {
    pub unsafe fn new(
        device: *mut c_void,
        target: *mut c_void,
        width: i32,
        height: i32,
        ten_bit: bool,
        hdr: bool,
    ) -> windows::core::Result<Self> {
        let target = ID3D11Texture2D::from_raw_borrowed(&target);
        let device = ID3D11Device::from_raw_borrowed(&device).unwrap();
        let context = device.GetImmediateContext()?;
        let mut deferred = None;
        device.CreateDeferredContext(0, Some(&mut deferred))?;
        let mut input = None;
        device.CreateTexture2D(
            &D3D11_TEXTURE2D_DESC {
                Width: width as u32,
                Height: height as u32,
                MipLevels: 1,
                ArraySize: 1,
                Format: if ten_bit {
                    DXGI_FORMAT_P010
                } else {
                    DXGI_FORMAT_NV12
                },
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                ..Default::default()
            },
            None,
            Some(&mut input),
        )?;
        let input = input.unwrap();
        let formats = if ten_bit {
            [DXGI_FORMAT_R16_UNORM, DXGI_FORMAT_R16G16_UNORM]
        } else {
            [DXGI_FORMAT_R8_UNORM, DXGI_FORMAT_R8G8_UNORM]
        };
        let mut planes = [None, None];
        for (i, format) in formats.into_iter().enumerate() {
            device.CreateShaderResourceView(
                &input,
                Some(&D3D11_SHADER_RESOURCE_VIEW_DESC {
                    Format: format,
                    ViewDimension: D3D_SRV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                        Texture2D: D3D11_TEX2D_SRV {
                            MostDetailedMip: 0,
                            MipLevels: 1,
                        },
                    },
                }),
                Some(&mut planes[i]),
            )?;
        }
        let mut render_target = None;
        if let Some(target) = target {
            device.CreateRenderTargetView(target, None, Some(&mut render_target))?;
        }
        let vertex_code = compile(include_bytes!("../packed444.hlsl"), s!("vertex"), s!("vs_5_0"))?;
        let mut vertex = None;
        device.CreateVertexShader(
            std::slice::from_raw_parts(
                vertex_code.GetBufferPointer().cast(),
                vertex_code.GetBufferSize(),
            ),
            None,
            Some(&mut vertex),
        )?;
        let fragment_code = if target.is_some() {
            compile(include_bytes!("../packed444.hlsl"), s!("fragment"), s!("ps_5_0"))?
        } else if hdr {
            compile(include_bytes!("../packed444.hlsl"), s!("present"), s!("ps_5_0"))?
        } else {
            let source = [b"#define SDR_PRESENT 1\n".as_slice(), include_bytes!("../packed444.hlsl")].concat();
            compile(&source, s!("present"), s!("ps_5_0"))?
        };
        let mut fragment = None;
        device.CreatePixelShader(
            std::slice::from_raw_parts(
                fragment_code.GetBufferPointer().cast(),
                fragment_code.GetBufferSize(),
            ),
            None,
            Some(&mut fragment),
        )?;
        let mut constants = None;
        device.CreateBuffer(
            &D3D11_BUFFER_DESC {
                ByteWidth: 48,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                ..Default::default()
            },
            None,
            Some(&mut constants),
        )?;
        let mut presentation = None;
        if target.is_none() {
            device.CreateBuffer(&D3D11_BUFFER_DESC {
                ByteWidth: PRESENTATION_BYTES, Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32, ..Default::default()
            }, None, Some(&mut presentation))?;
        }
        Ok(Self {
            context,
            deferred: deferred.unwrap(),
            input,
            planes,
            target: [render_target],
            vertex: vertex.unwrap(),
            fragment: fragment.unwrap(),
            constants: [constants],
            presentation: [presentation],
            width,
            height,
            back_buffer: Default::default(),
            device: device.clone(),
            mask: Default::default(),
        })
    }

    /// Call before SDL runs its queue: a back buffer view for another swap chain or output size goes.
    pub fn keep_back_buffer_if(&self, swap_chain: usize, size: (i32, i32)) {
        self.back_buffer.borrow_mut().keep_if(swap_chain, size);
    }

    /// Lets go of the back buffer view, for a frame that does not present with it (SDL may resize).
    pub fn release_back_buffer(&self) {
        self.back_buffer.borrow_mut().release();
    }

    pub fn draw(&self, frame: &media::Frame, parameters: &[u32; 12]) -> bool {
        self.upload(frame) && self.render(&self.target, parameters,
            &[0.0, 0.0, parameters[0] as f32, parameters[1] as f32], None)
    }

    pub fn upload(&self, frame: &media::Frame) -> bool {
        frame.copy_to_d3d11(self.input.as_raw(), self.width, self.height)
    }

    /// Draws into the back buffer of `swapchain`, `output` pixels large. The view of it is kept
    /// between frames; the caller calls `keep_back_buffer_if` before SDL's queue runs and
    /// `release_back_buffer` for a frame that does not present here, so SDL can resize.
    /// `warp` is Frame Warp's shift for the draw (`packed444.hlsl`'s `frameWarp`).
    #[allow(clippy::too_many_arguments)] // the presentation's own values, passed as they are
    pub unsafe fn present(&self, swapchain: *mut c_void, output: (i32, i32), parameters: &[u32; 12], destination: &[f32; 4], linear: bool, scale: f32,
        warp: &super::frame_warp::WarpDraw) -> bool {
        let Ok(target) = self.back_buffer.borrow_mut().view(swapchain, output) else { return false; };
        let mask = self.mask.borrow_mut().view(&self.device, warp);
        let (row, lens) = (warp.row(mask.is_some()), warp.lens_row());
        let presentation = [destination[0], destination[1], destination[2], destination[3], u32::from(linear) as f32, scale, 0.0, 0.0,
            row[0], row[1], row[2], row[3], lens[0], lens[1], lens[2], lens[3]];
        let x = destination[0].max(0.0);
        let y = destination[1].max(0.0);
        let clipped = [x, y, (destination[0] + destination[2]).min(output.0 as f32) - x,
            (destination[1] + destination[3]).min(output.1 as f32) - y];
        self.render(&[Some(target)], parameters, &clipped, Some((&presentation, mask)))
    }

    fn render(&self, target: &[Option<ID3D11RenderTargetView>; 1], parameters: &[u32; 12], destination: &[f32; 4],
        presentation: Option<(&[f32; PRESENTATION_FLOATS], Option<ID3D11ShaderResourceView>)>) -> bool {
        unsafe {
            // Recording on a deferred context preserves SDL's cached immediate-context state.
            let c = &self.deferred;
            c.ClearState();
            c.UpdateSubresource(
                self.constants[0].as_ref().unwrap(),
                0,
                None,
                parameters.as_ptr().cast(),
                0,
                0,
            );
            c.OMSetRenderTargets(Some(target), None);
            c.RSSetViewports(Some(&[D3D11_VIEWPORT {
                TopLeftX: destination[0],
                TopLeftY: destination[1],
                Width: destination[2],
                Height: destination[3],
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]));
            c.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            c.VSSetShader(&self.vertex, None);
            c.PSSetShader(&self.fragment, None);
            c.PSSetShaderResources(0, Some(&self.planes));
            c.PSSetConstantBuffers(0, Some(&self.constants));
            if let Some((presentation, mask)) = presentation {
                c.UpdateSubresource(self.presentation[0].as_ref().unwrap(), 0, None, presentation.as_ptr().cast(), 0, 0);
                c.PSSetConstantBuffers(1, Some(&self.presentation));
                if mask.is_some() { c.PSSetShaderResources(2, Some(&[mask])); }
            }
            c.Draw(3, 0);
            let mut commands = None;
            if c.FinishCommandList(false, Some(&mut commands)).is_err() {
                return false;
            }
            self.context
                .ExecuteCommandList(commands.as_ref().unwrap(), true);
        }
        true
    }
}

#[cfg(test)]
mod tests;
