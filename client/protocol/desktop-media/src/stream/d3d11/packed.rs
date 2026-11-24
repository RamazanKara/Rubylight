//! Direct3D 11 packed 4:4:4 NV12/P010 conversion and back-buffer presentation pass (`Packed`).

use std::ffi::c_void;

use windows::core::{s, Interface};
use windows::Win32::Graphics::Direct3D::{D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, D3D_SRV_DIMENSION_TEXTURE2D};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use crate::media;
use super::{compile, create_dynamic_cb, write_cb, BackBuffer};

/// `packed444.hlsl`'s `Presentation` block: the destination, the output's colour, then Frame
/// Warp's two rows.
const PRESENTATION_FLOATS: usize = 16;
pub(super) const PRESENTATION_BYTES: u32 = PRESENTATION_FLOATS as u32 * 4;

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
    mask: std::cell::RefCell<super::super::frame_warp::D3d11Mask>,
}

impl Packed {
    /// # Safety
    /// `device` must be a live `ID3D11Device` pointer, and `target` must be null or a live
    /// `ID3D11Texture2D` pointer on `device`.
    pub unsafe fn new(
        device: *mut c_void,
        target: *mut c_void,
        width: i32,
        height: i32,
        ten_bit: bool,
        hdr: bool,
    ) -> windows::core::Result<Self> {
        // SAFETY: caller guarantees `device` and non-null `target` are live D3D11 objects.
        unsafe {
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
            let vertex_code = compile(include_bytes!("../../packed444.hlsl"), s!("vertex"), s!("vs_5_0"))?;
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
                compile(include_bytes!("../../packed444.hlsl"), s!("fragment"), s!("ps_5_0"))?
            } else if hdr {
                compile(include_bytes!("../../packed444.hlsl"), s!("present"), s!("ps_5_0"))?
            } else {
                let source = [b"#define SDR_PRESENT 1\n".as_slice(), include_bytes!("../../packed444.hlsl")].concat();
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
            let constants = Some(create_dynamic_cb(device, 48)?);
            let presentation = if target.is_none() {
                Some(create_dynamic_cb(device, PRESENTATION_BYTES)?)
            } else {
                None
            };
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
    ///
    /// # Safety
    /// `swapchain` must be the live `IDXGISwapChain` on this thread.
    #[allow(clippy::too_many_arguments)] // the presentation's own values, passed as they are
    pub unsafe fn present(&self, swapchain: *mut c_void, output: (i32, i32), parameters: &[u32; 12], destination: &[f32; 4], linear: bool, scale: f32,
        warp: &super::super::frame_warp::WarpDraw) -> bool {
        // SAFETY: caller guarantees `swapchain` is the live swap chain on this thread.
        let Ok(target) = (unsafe { self.back_buffer.borrow_mut().view(swapchain, output) }) else { return false; };
        // SAFETY: `self.device` is live on this thread.
        let mask = unsafe { self.mask.borrow_mut().view(&self.device, warp) };
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
            if write_cb(c, self.constants[0].as_ref().unwrap(), parameters).is_err() { return false; }
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
                if write_cb(c, self.presentation[0].as_ref().unwrap(), presentation).is_err() { return false; }
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
