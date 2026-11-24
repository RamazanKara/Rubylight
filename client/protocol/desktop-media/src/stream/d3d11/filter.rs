//! Deferred-context Direct3D 11 full-screen filter pass (`Filter`, `FilterStep`).

use std::ffi::c_void;

use windows::core::{s, Interface};
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D11::*;

use super::{compile, create_dynamic_cb, write_cb, FilterStep};

pub struct Filter {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    deferred: ID3D11DeviceContext,
    vertex: ID3D11VertexShader,
    fragment: ID3D11PixelShader,
    constants: [Option<ID3D11Buffer>; 1],
    sampler: [Option<ID3D11SamplerState>; 1],
    bindings: Vec<Binding>,
}

/// The textures one draw of a [`Filter`] reads (slots t0 and on) and the one it renders to.
struct Binding {
    inputs: Vec<Option<ID3D11ShaderResourceView>>,
    target: [Option<ID3D11RenderTargetView>; 1],
    size: (u32, u32),
}

impl Filter {
    /// # Safety
    /// `input` and `target` must be live `ID3D11Texture2D` pointers on the same device.
    pub unsafe fn new(input: *mut c_void, target: *mut c_void, source: &[u8]) -> windows::core::Result<Self> {
        // SAFETY: caller guarantees `input` and `target` are live `ID3D11Texture2D` pointers on the same device.
        unsafe {
            let mut filter = Self::compiled(target, source)?;
            filter.bind(&[input], target)?;
            Ok(filter)
        }
    }

    /// The filter's shaders on the device of `texture`, without textures yet (`bind`).
    ///
    /// # Safety
    /// `texture` must be a live `ID3D11Texture2D` pointer.
    pub unsafe fn compiled(texture: *mut c_void, source: &[u8]) -> windows::core::Result<Self> {
        // SAFETY: caller guarantees `texture` is a live `ID3D11Texture2D` pointer.
        unsafe {
            let texture = ID3D11Texture2D::from_raw_borrowed(&texture).unwrap();
            let device = texture.GetDevice()?;
            let context = device.GetImmediateContext()?;
            let mut deferred = None;
            device.CreateDeferredContext(0, Some(&mut deferred))?;
            let code = compile(source, s!("vertex"), s!("vs_5_0"))?;
            let mut vertex = None;
            device.CreateVertexShader(std::slice::from_raw_parts(code.GetBufferPointer().cast(), code.GetBufferSize()), None, Some(&mut vertex))?;
            let code = compile(source, s!("fragment"), s!("ps_5_0"))?;
            let mut fragment = None;
            device.CreatePixelShader(std::slice::from_raw_parts(code.GetBufferPointer().cast(), code.GetBufferSize()), None, Some(&mut fragment))?;
            let constants = Some(create_dynamic_cb(&device, 32)?);
            let mut sampler = None;
            device.CreateSamplerState(&D3D11_SAMPLER_DESC { Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP, AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP, MaxLOD: f32::MAX, ..Default::default() }, Some(&mut sampler))?;
            Ok(Self { device, context, deferred: deferred.unwrap(), vertex: vertex.unwrap(), fragment: fragment.unwrap(),
                constants: [constants], sampler: [sampler], bindings: Vec::new() })
        }
    }

    /// Adds a draw reading `inputs` in slots t0 and on into `target` (textures on the filter's
    /// device), drawn by its index in the order bound (`draw_binding`).
    ///
    /// # Safety
    /// `inputs` and `target` must be live `ID3D11Texture2D` pointers on `self.device`.
    pub unsafe fn bind(&mut self, inputs: &[*mut c_void], target: *mut c_void) -> windows::core::Result<()> {
        // SAFETY: caller guarantees `inputs` and `target` are live `ID3D11Texture2D` pointers on `self.device`.
        unsafe {
            let target = ID3D11Texture2D::from_raw_borrowed(&target).unwrap();
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            target.GetDesc(&mut desc);
            let mut views = Vec::with_capacity(inputs.len());
            for input in inputs {
                let mut view = None;
                self.device.CreateShaderResourceView(ID3D11Texture2D::from_raw_borrowed(input).unwrap(), None, Some(&mut view))?;
                views.push(view);
            }
            let mut target_view = None;
            self.device.CreateRenderTargetView(target, None, Some(&mut target_view))?;
            self.bindings.push(Binding { inputs: views, target: [target_view], size: (desc.Width, desc.Height) });
            Ok(())
        }
    }

    pub fn draw(&self, parameters: &[u32; 8], area: Option<(u32, u32)>) -> bool {
        self.draw_binding(0, parameters, area)
    }

    pub fn draw_binding(&self, binding: usize, parameters: &[u32; 8], area: Option<(u32, u32)>) -> bool {
        self.draw_batch(&[(binding, *parameters, area)])
    }

    /// Records every `(binding, parameters, area)` step into one deferred command list and
    /// submits it with a single `ExecuteCommandList`, unbinding SRVs before RTV swaps.
    pub fn draw_batch(&self, steps: &[FilterStep]) -> bool {
        if steps.is_empty() { return true; }
        let cb = self.constants[0].as_ref().unwrap();
        let clear_srvs: [Option<ID3D11ShaderResourceView>; 4] = [None, None, None, None];
        // SAFETY: `self.deferred` and `self.context` belong to the presenting thread; every binding lives on `self.device`.
        unsafe {
            let c = &self.deferred;
            c.ClearState();
            c.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            c.VSSetShader(&self.vertex, None);
            c.PSSetShader(&self.fragment, None);
            c.PSSetSamplers(0, Some(&self.sampler));
            c.PSSetConstantBuffers(0, Some(&self.constants));
            for &(index, ref parameters, area) in steps {
                let Some(binding) = self.bindings.get(index) else { return false };
                let size = area.unwrap_or(binding.size);
                if write_cb(c, cb, parameters).is_err() { return false; }
                let clear = binding.inputs.len().clamp(1, clear_srvs.len());
                c.PSSetShaderResources(0, Some(&clear_srvs[..clear]));
                c.OMSetRenderTargets(Some(&binding.target), None);
                c.RSSetViewports(Some(&[D3D11_VIEWPORT {
                    Width: size.0 as f32, Height: size.1 as f32, MinDepth: 0.0, MaxDepth: 1.0, ..Default::default()
                }]));
                c.PSSetShaderResources(0, Some(&binding.inputs));
                c.Draw(3, 0);
            }
            let mut commands = None;
            if c.FinishCommandList(false, Some(&mut commands)).is_err() { return false; }
            self.context.ExecuteCommandList(commands.as_ref().unwrap(), true);
            true
        }
    }
}
