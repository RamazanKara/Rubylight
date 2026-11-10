use std::ffi::c_void;

use windows::core::{s, Interface, PCSTR};
use windows::Win32::Graphics::Direct3D::{
    Fxc::*, ID3DBlob, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, D3D_SRV_DIMENSION_TEXTURE2D,
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
}

/// The source of the plane pass with the 4:4:4 surface entry (`surface444.hlsl`) appended.
fn surface_source() -> Vec<u8> {
    [include_bytes!("../pyrowave.hlsl").as_slice(), b"\n", include_bytes!("../surface444.hlsl")].concat()
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
            surface: None, surface_fragment: None })
    }

    /// Compiles the plane shader for a probe: whether the pass can be built at all.
    pub fn compiles() -> Result<(), String> {
        let source = include_bytes!("../pyrowave.hlsl");
        // SAFETY: Plain compiler calls on static source.
        unsafe {
            compile(source, s!("vertex"), s!("vs_5_0")).map_err(|e| format!("the PyroWave plane shader does not compile: {e}"))?;
            compile(source, s!("planes"), s!("ps_5_0")).map_err(|e| format!("the PyroWave plane shader does not compile: {e}"))?;
            compile(&surface_source(), s!("surface444"), s!("ps_5_0")).map_err(|e| format!("the 4:4:4 picture shader does not compile: {e}"))?;
        }
        Ok(())
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
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: A live texture.
        unsafe { source.GetDesc(&mut desc) };
        let Some((view_format, _)) = surface_view(desc.Format) else {
            return Err(format!("the decoder's pictures are DXGI format {}, not a 4:4:4 format this pass reads", desc.Format.0));
        };
        let parameters = surface_parameters(*parameters, desc.Format).expect("a format surface_view reads");
        if size.0 == 0 || size.1 == 0 || size.0 > desc.Width || size.1 > desc.Height || slice >= desc.ArraySize {
            return Err(format!("a {}x{} picture in slice {slice} does not fit the decoder's {}x{}x{} textures", size.0, size.1, desc.Width, desc.Height, desc.ArraySize));
        }
        // SAFETY: A live texture's device.
        let device = unsafe { source.GetDevice() }.map_err(|e| format!("the decoder's texture has no device: {e}"))?;
        if device.as_raw() != self.device.as_raw() {
            return Err("the decoder's pictures are on another Direct3D 11 device than the renderer's".into());
        }
        if self.surface.as_ref().is_none_or(|copy| copy.format != desc.Format || copy.size != size) {
            self.surface = None;
            // SAFETY: A texture and view on the pass's own device.
            self.surface = Some(unsafe { SurfaceCopy::new(&self.device, desc.Format, view_format, size) }
                .map_err(|e| format!("cannot make the {}x{} copy of the decoder's 4:4:4 pictures: {e}", size.0, size.1))?);
        }
        let fragment = self.surface_fragment()?;
        let copy = self.surface.as_ref().unwrap();
        let region = D3D11_BOX { left: 0, top: 0, front: 0, right: size.0, bottom: size.1, back: 1 };
        // Subresource `slice` is the picture: the decoder's textures have one mip level.
        let subresource = slice * desc.MipLevels.max(1);
        // SAFETY: Both textures are live on this device, the region is inside both, and the
        // immediate context is used under the lock FFmpeg holds while it decodes.
        lock(&mut || unsafe { self.context.CopySubresourceRegion(&copy.texture, 0, 0, 0, 0, source, subresource, Some(&region)) });
        let view = copy.view.clone();
        self.record_pass(&fragment, 4, &view, &parameters, None).map_err(|e| format!("cannot record the 4:4:4 picture pass: {e}"))
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
            self.context.ExecuteCommandList(commands.as_ref().unwrap(), true);
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
}

pub(crate) unsafe fn compile(source: &[u8], entry: PCSTR, profile: PCSTR) -> windows::core::Result<ID3DBlob> {
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
                ByteWidth: 32, Usage: D3D11_USAGE_DEFAULT,
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
        })
    }

    pub fn draw(&self, frame: &media::Frame, parameters: &[u32; 12]) -> bool {
        self.upload(frame) && self.render(&self.target, parameters,
            &[0.0, 0.0, parameters[0] as f32, parameters[1] as f32], None)
    }

    pub fn upload(&self, frame: &media::Frame) -> bool {
        frame.copy_to_d3d11(self.input.as_raw(), self.width, self.height)
    }

    pub unsafe fn present(&self, swapchain: *mut c_void, parameters: &[u32; 12], destination: &[f32; 4], linear: bool, scale: f32) -> bool {
        let swapchain = IDXGISwapChain::from_raw_borrowed(&swapchain).unwrap();
        let Ok(buffer) = swapchain.GetBuffer::<ID3D11Texture2D>(0) else { return false; };
        let Ok(device) = buffer.GetDevice() else { return false; };
        let mut target = None;
        if device.CreateRenderTargetView(&buffer, None, Some(&mut target)).is_err() { return false; }
        let presentation = [destination[0], destination[1], destination[2], destination[3], u32::from(linear) as f32, scale, 0.0, 0.0];
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        buffer.GetDesc(&mut desc);
        let x = destination[0].max(0.0);
        let y = destination[1].max(0.0);
        let clipped = [x, y, (destination[0] + destination[2]).min(desc.Width as f32) - x,
            (destination[1] + destination[3]).min(desc.Height as f32) - y];
        // Release the swapchain buffer and view before SDL next resizes the window.
        self.render(&[target], parameters, &clipped, Some(&presentation))
    }

    fn render(&self, target: &[Option<ID3D11RenderTargetView>; 1], parameters: &[u32; 12], destination: &[f32; 4], presentation: Option<&[f32; 8]>) -> bool {
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
            if let Some(presentation) = presentation {
                c.UpdateSubresource(self.presentation[0].as_ref().unwrap(), 0, None, presentation.as_ptr().cast(), 0, 0);
                c.PSSetConstantBuffers(1, Some(&self.presentation));
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
mod tests {
    use super::after_slice_draw;
    use std::cell::Cell;

    /// pyrowave.hlsl's Vulkan-only parts (Vulkan video's interleaved chroma) leave the plane shader
    /// Direct3D compiles as it was.
    #[test]
    fn the_plane_shader_compiles_for_direct3d() {
        assert_eq!(super::PlanesPass::compiles(), Ok(()));
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
            let status = decoder.send(include_bytes!("../../tests/fixtures/hevc-444-10.bin"), 0);
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
}
