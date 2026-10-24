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
            raster: raster.unwrap(), size: (desc.Width, desc.Height), views: Default::default(), owner: None })
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
        self.record(&planes, pin.set(), fence_value, parameters, deadline, Some(rows))?;
        self.signal(&planes, value)
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
        let (top, bottom) = rows.map_or((0, self.size.1), |(first, end)| (first.min(self.size.1), end.min(self.size.1)));
        // SAFETY: The immediate context belongs to this (the presenting) thread; the deferred
        // context records the draw so SDL's cached immediate-context state is restored.
        unsafe {
            self.queue.Wait(planes.decode_fence(), fence_value).map_err(|e| format!("cannot order the picture after the PyroWave decode: {e}"))?;
            let c = &self.deferred;
            c.ClearState();
            c.UpdateSubresource(self.constants[0].as_ref().unwrap(), 0, None, parameters.as_ptr().cast(), 0, 0);
            c.OMSetRenderTargets(Some(&self.target), None);
            c.RSSetState(&self.raster);
            c.RSSetScissorRects(Some(&[windows::Win32::Foundation::RECT { left: 0, top: top as i32, right: self.size.0 as i32, bottom: bottom as i32 }]));
            c.RSSetViewports(Some(&[D3D11_VIEWPORT { Width: self.size.0 as f32, Height: self.size.1 as f32,
                MinDepth: 0.0, MaxDepth: 1.0, ..Default::default() }]));
            c.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            c.VSSetShader(&self.vertex, None);
            c.PSSetShader(&self.fragment, None);
            c.PSSetShaderResources(1, Some(&views));
            c.PSSetSamplers(0, Some(&self.sampler));
            c.PSSetConstantBuffers(0, Some(&self.constants));
            c.Draw(3, 0);
            let mut commands = None;
            c.FinishCommandList(false, Some(&mut commands)).map_err(|e| format!("cannot record the PyroWave plane pass: {e}"))?;
            self.context.ExecuteCommandList(commands.as_ref().unwrap(), true);
        }
        Ok(())
    }
}

/// Gives the plane set back as `value` on the reuse fence. The set goes back also when the signal
/// fails: the draw is queued and the error ends the stream, so the set must not stay held.
pub(crate) fn hand_back(lease: &Lease, value: u64, signal: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    let signalled = signal();
    lease.release_after(value);
    signalled
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
