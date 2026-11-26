//! Direct3D 11 packed 4:4:4 NV12/P010 conversion and back-buffer presentation pass (`Packed`).

use std::cell::RefCell;
use std::ffi::c_void;

use windows::core::{s, Interface};
use windows::Win32::Foundation::E_INVALIDARG;
use windows::Win32::Graphics::Direct3D::{D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, D3D_SRV_DIMENSION_TEXTURE2D};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use crate::media;
use super::{compile, create_dynamic_cb, write_cb, BackBuffer};

/// `packed444.hlsl`'s `Presentation` block: the destination, the output's colour, then Frame
/// Warp's three rows.
const PRESENTATION_FLOATS: usize = 20;
pub(super) const PRESENTATION_BYTES: u32 = PRESENTATION_FLOATS as u32 * 4;
const MAX_SHARED_VIEWS: usize = 32;

struct CopyInput {
    texture: ID3D11Texture2D,
    planes: [Option<ID3D11ShaderResourceView>; 2],
}

#[derive(Default)]
struct PackedInput {
    copy_input: Option<CopyInput>,
    held: Option<(media::Frame, [Option<ID3D11ShaderResourceView>; 2])>,
    shared_bridge_id: Option<u64>,
    shared_views: Vec<(usize, ID3D11Texture2D, [Option<ID3D11ShaderResourceView>; 2])>,
    #[cfg(test)]
    force_copy: bool,
}

unsafe fn create_plane_views(
    device: &ID3D11Device,
    texture: &ID3D11Texture2D,
    ten_bit: bool,
) -> windows::core::Result<[Option<ID3D11ShaderResourceView>; 2]> {
    let formats = if ten_bit { [DXGI_FORMAT_R16_UNORM, DXGI_FORMAT_R16G16_UNORM] } else { [DXGI_FORMAT_R8_UNORM, DXGI_FORMAT_R8G8_UNORM] };
    let mut planes = [None, None];
    for (i, format) in formats.into_iter().enumerate() {
        let desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
            Format: format, ViewDimension: D3D_SRV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_SRV { MostDetailedMip: 0, MipLevels: 1 } },
        };
        // SAFETY: caller guarantees `device` and `texture` are live D3D11 objects with `SHADER_RESOURCE`.
        unsafe { device.CreateShaderResourceView(texture, Some(&desc), Some(&mut planes[i]))? };
    }
    Ok(planes)
}

impl PackedInput {
    fn ensure_copy_input(&mut self, device: &ID3D11Device, width: i32, height: i32, ten_bit: bool) -> Option<&CopyInput> {
        if self.copy_input.is_none() {
            if width <= 0 || height <= 0 { return None; }
            let desc = D3D11_TEXTURE2D_DESC {
                Width: width as u32, Height: height as u32, MipLevels: 1, ArraySize: 1,
                Format: if ten_bit { DXGI_FORMAT_P010 } else { DXGI_FORMAT_NV12 },
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32, ..Default::default()
            };
            let mut texture = None;
            // SAFETY: `device` is a live D3D11 device on the presenting thread.
            unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture)) }.ok()?;
            let texture = texture?;
            // SAFETY: `texture` was created on `device` with `D3D11_BIND_SHADER_RESOURCE`.
            let planes = unsafe { create_plane_views(device, &texture, ten_bit) }.ok()?;
            self.copy_input = Some(CopyInput { texture, planes });
        }
        self.copy_input.as_ref()
    }
}

pub struct Packed {
    context: ID3D11DeviceContext,
    deferred: ID3D11DeviceContext,
    target: [Option<ID3D11RenderTargetView>; 1],
    vertex: ID3D11VertexShader,
    fragment: ID3D11PixelShader,
    constants: [Option<ID3D11Buffer>; 1],
    presentation: [Option<ID3D11Buffer>; 1],
    width: i32,
    height: i32,
    ten_bit: bool,
    /// SDL's back buffer as a render target, kept across frames (see `BackBuffer`).
    back_buffer: RefCell<BackBuffer>,
    device: ID3D11Device,
    /// Frame Warp's HUD mask for `present` (`t2`).
    mask: RefCell<super::super::frame_warp::D3d11Mask>,
    input: RefCell<PackedInput>,
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
        if width <= 0 || height <= 0 {
            return Err(E_INVALIDARG.into());
        }
        // SAFETY: caller guarantees `device` and non-null `target` are live D3D11 objects.
        unsafe {
            let target = ID3D11Texture2D::from_raw_borrowed(&target);
            let device = ID3D11Device::from_raw_borrowed(&device).unwrap();
            let context = device.GetImmediateContext()?;
            let mut deferred = None;
            device.CreateDeferredContext(0, Some(&mut deferred))?;
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
                target: [render_target],
                vertex: vertex.unwrap(),
                fragment: fragment.unwrap(),
                constants: [constants],
                presentation: [presentation],
                width,
                height,
                ten_bit,
                back_buffer: Default::default(),
                device: device.clone(),
                mask: Default::default(),
                input: Default::default(),
            })
        }
    }

    #[cfg(test)]
    pub(super) fn has_copy_input(&self) -> bool {
        self.input.borrow().copy_input.is_some()
    }

    #[cfg(test)]
    pub(super) fn has_held_frame(&self) -> bool {
        self.input.borrow().held.is_some()
    }

    #[cfg(test)]
    pub(super) fn cached_shared_views_len(&self) -> usize {
        self.input.borrow().shared_views.len()
    }

    #[cfg(test)]
    pub(super) fn set_force_copy(&self, force: bool) {
        self.input.borrow_mut().force_copy = force;
    }

    #[cfg(test)]
    pub(super) fn can_share_views(&self, frame: &media::Frame) -> bool {
        let mut state = self.input.borrow_mut();
        self.shared_views(&mut state, frame).is_some()
    }

    fn shared_views(&self, state: &mut PackedInput, frame: &media::Frame) -> Option<[Option<ID3D11ShaderResourceView>; 2]> {
        #[cfg(test)]
        if state.force_copy { return None; }
        if !frame.is_d3d11_bridge() || self.width <= 0 || self.height <= 0 { return None; }
        let bridge_id = frame.d3d11_bridge_id()?;
        if state.shared_bridge_id != Some(bridge_id) {
            state.shared_views.clear();
            state.shared_bridge_id = Some(bridge_id);
        }
        let surface = frame.d3d11_surface()?;
        if !surface.readable || surface.slice != 0 { return None; }
        let key = surface.texture as usize;
        if let Some((_, _, views)) = state.shared_views.iter().find(|(cached, _, _)| *cached == key) {
            return Some(views.clone());
        }
        // SAFETY: `surface.texture` is a live `ID3D11Texture2D` kept alive by `frame`.
        let source = unsafe { ID3D11Texture2D::from_raw_borrowed(&surface.texture) }?;
        // SAFETY: `source` is a live `ID3D11Texture2D`.
        if unsafe { source.GetDevice() }.ok()?.as_raw() != self.device.as_raw() { return None; }
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: `source` is a live `ID3D11Texture2D`.
        unsafe { source.GetDesc(&mut desc) };
        let expected = if self.ten_bit { DXGI_FORMAT_P010 } else { DXGI_FORMAT_NV12 };
        if desc.ArraySize != 1 || desc.Format != expected || desc.Width < self.width as u32
            || desc.Height < self.height as u32 || desc.BindFlags & D3D11_BIND_SHADER_RESOURCE.0 as u32 == 0 {
            return None;
        }
        // SAFETY: `source` belongs to `self.device` and passed format, geometry, and bind checks.
        let views = unsafe { create_plane_views(&self.device, source, self.ten_bit) }.ok()?;
        if state.shared_views.len() >= MAX_SHARED_VIEWS { state.shared_views.remove(0); }
        state.shared_views.push((key, source.clone(), views.clone()));
        Some(views)
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
        let rect = [0.0, 0.0, parameters[0] as f32, parameters[1] as f32];
        let bridged = {
            let mut state = self.input.borrow_mut();
            let views = self.shared_views(&mut state, frame);
            if views.is_some() {
                state.held = None;
            }
            views
        };
        if let Some(views) = bridged {
            return self.render(&self.target, parameters, &rect, None, &views, Some(frame));
        }
        if !self.upload(frame) {
            return false;
        }
        let planes = {
            let state = self.input.borrow();
            let Some(copy) = &state.copy_input else {
                return false;
            };
            copy.planes.clone()
        };
        self.render(&self.target, parameters, &rect, None, &planes, None)
    }

    pub fn upload(&self, frame: &media::Frame) -> bool {
        let copy_tex = {
            let mut state = self.input.borrow_mut();
            if let Some(views) = self.shared_views(&mut state, frame)
                && let Some(held) = frame.reference()
            {
                state.held = Some((held, views));
                return true;
            }
            state.held = None;
            let Some(copy) = state.ensure_copy_input(&self.device, self.width, self.height, self.ten_bit) else {
                return false;
            };
            copy.texture.as_raw()
        };
        frame.copy_to_d3d11(copy_tex, self.width, self.height)
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
        self.present_target(&target, output, parameters, destination, linear, scale, warp)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn present_target(&self, target: &ID3D11RenderTargetView, output: (i32, i32), parameters: &[u32; 12], destination: &[f32; 4], linear: bool, scale: f32,
        warp: &super::super::frame_warp::WarpDraw) -> bool {
        // SAFETY: `self.device` is live on this thread.
        let mask = unsafe { self.mask.borrow_mut().view(&self.device, warp) };
        let (row, lens, v2) = (warp.row(mask.is_some()), warp.lens_row(), warp.v2_row());
        let presentation = [destination[0], destination[1], destination[2], destination[3], u32::from(linear) as f32, scale, 0.0, 0.0,
            row[0], row[1], row[2], row[3], lens[0], lens[1], lens[2], lens[3], v2[0], v2[1], v2[2], v2[3]];
        let x = destination[0].max(0.0);
        let y = destination[1].max(0.0);
        let clipped = [x, y, (destination[0] + destination[2]).min(output.0 as f32) - x,
            (destination[1] + destination[3]).min(output.1 as f32) - y];
        let state = self.input.borrow();
        if let Some((frame, views)) = &state.held {
            self.render(&[Some(target.clone())], parameters, &clipped, Some((&presentation, mask)), views, Some(frame))
        } else if let Some(copy) = &state.copy_input {
            self.render(&[Some(target.clone())], parameters, &clipped, Some((&presentation, mask)), &copy.planes, None)
        } else {
            false
        }
    }

    fn render(&self, target: &[Option<ID3D11RenderTargetView>; 1], parameters: &[u32; 12], destination: &[f32; 4],
        presentation: Option<(&[f32; PRESENTATION_FLOATS], Option<ID3D11ShaderResourceView>)>,
        planes: &[Option<ID3D11ShaderResourceView>; 2], lock_frame: Option<&media::Frame>) -> bool {
        if let Some(frame) = lock_frame
            && frame.wait_d3d11_ready().is_err()
        {
            return false;
        }
        unsafe {
            // Recording on a deferred context preserves SDL's cached immediate-context state.
            let c = &self.deferred;
            c.ClearState();
            if write_cb(c, self.constants[0].as_ref().unwrap(), parameters).is_err() {
                c.ClearState();
                return false;
            }
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
            c.PSSetShaderResources(0, Some(planes));
            c.PSSetConstantBuffers(0, Some(&self.constants));
            if let Some((presentation, mask)) = presentation {
                if write_cb(c, self.presentation[0].as_ref().unwrap(), presentation).is_err() {
                    c.ClearState();
                    return false;
                }
                c.PSSetConstantBuffers(1, Some(&self.presentation));
                if mask.is_some() { c.PSSetShaderResources(2, Some(&[mask])); }
            }
            c.Draw(3, 0);
            let mut commands = None;
            if c.FinishCommandList(false, Some(&mut commands)).is_err() {
                c.ClearState();
                return false;
            }
            let commands = commands.as_ref().unwrap();
            if let Some(frame) = lock_frame {
                let mut executed = false;
                frame.with_decoder_lock(&mut || {
                    self.context.ExecuteCommandList(commands, true);
                    executed = true;
                });
                return executed;
            }
            self.context.ExecuteCommandList(commands, true);
        }
        true
    }
}
