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
            raster: raster.unwrap(), size: (desc.Width, desc.Height), views: Default::default(), owner: None, frame: None })
    }

    /// Compiles the plane shader for a probe: whether the pass can be built at all.
    pub fn compiles() -> Result<(), String> {
        let source = include_bytes!("../pyrowave.hlsl");
        // SAFETY: Plain compiler calls on static source.
        unsafe {
            compile(source, s!("vertex"), s!("vs_5_0")).map_err(|e| format!("the PyroWave plane shader does not compile: {e}"))?;
            compile(source, s!("planes"), s!("ps_5_0")).map_err(|e| format!("the PyroWave plane shader does not compile: {e}"))?;
        }
        Ok(())
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
            c.PSSetShader(&self.fragment, None);
            c.PSSetShaderResources(1, Some(views));
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

unsafe fn compile(source: &[u8], entry: PCSTR, profile: PCSTR) -> windows::core::Result<ID3DBlob> {
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
            let mut staging = None;
            device.CreateTexture2D(&D3D11_TEXTURE2D_DESC { Usage: D3D11_USAGE_STAGING, BindFlags: 0, CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32, ..desc },
                None, Some(&mut staging)).unwrap();
            let staging = staging.unwrap();
            context.CopyResource(&staging, &target);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
            let mut pixels = Vec::new();
            for y in 0..height as usize {
                let row = std::slice::from_raw_parts(mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize).cast::<u16>(), width as usize * 4);
                pixels.extend(row.chunks(4).map(|p| [from_half(p[0]), from_half(p[1]), from_half(p[2])]));
            }
            context.Unmap(&staging, 0);
            pixels
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
            let pixels = draw_on_warp(&info, &crate::stream::present::planar_parameters(&info, layout, false, 1.0));
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
