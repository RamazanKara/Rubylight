use std::ffi::c_void;

use windows::core::{s, Interface, PCSTR};
use windows::Win32::Graphics::Direct3D::{
    Fxc::*, ID3DBlob, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, D3D_SRV_DIMENSION_TEXTURE2D,
};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use crate::media;

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

pub struct Packed {
    context: ID3D11DeviceContext,
    deferred: ID3D11DeviceContext,
    input: ID3D11Texture2D,
    planes: [Option<ID3D11ShaderResourceView>; 2],
    target: [Option<ID3D11RenderTargetView>; 1],
    vertex: ID3D11VertexShader,
    fragment: ID3D11PixelShader,
    constants: [Option<ID3D11Buffer>; 1],
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
        target: *mut c_void,
        width: i32,
        height: i32,
        ten_bit: bool,
    ) -> windows::core::Result<Self> {
        let target = ID3D11Texture2D::from_raw_borrowed(&target).unwrap();
        let device = target.GetDevice()?;
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
        device.CreateRenderTargetView(target, None, Some(&mut render_target))?;
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
        let fragment_code = compile(include_bytes!("../packed444.hlsl"), s!("fragment"), s!("ps_5_0"))?;
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
        Ok(Self {
            context,
            deferred: deferred.unwrap(),
            input,
            planes,
            target: [render_target],
            vertex: vertex.unwrap(),
            fragment: fragment.unwrap(),
            constants: [constants],
            width,
            height,
        })
    }

    pub fn draw(&self, frame: &media::Frame, parameters: &[u32; 12]) -> bool {
        if !frame.copy_to_d3d11(self.input.as_raw(), self.width, self.height) {
            return false;
        }
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
            c.OMSetRenderTargets(Some(&self.target), None);
            c.RSSetViewports(Some(&[D3D11_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: parameters[0] as f32,
                Height: parameters[1] as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]));
            c.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            c.VSSetShader(&self.vertex, None);
            c.PSSetShader(&self.fragment, None);
            c.PSSetShaderResources(0, Some(&self.planes));
            c.PSSetConstantBuffers(0, Some(&self.constants));
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
