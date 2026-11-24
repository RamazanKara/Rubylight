//! Direct3D 11 target views (`BackBuffer`), plane/surface texture holders, shader source builders,
//! and plane-set handback helpers.

use std::ffi::c_void;

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D::D3D_SRV_DIMENSION_TEXTURE2D;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::IDXGISwapChain;

use crate::media;
use super::super::gpu_planes::Lease;

/// The pixel shaders of the direct draw (planes_direct.hlsl) and its constants (b1).
pub(super) struct DirectShaders {
    pub(super) planes: ID3D11PixelShader,
    pub(super) surface: ID3D11PixelShader,
    pub(super) surface_array: ID3D11PixelShader,
    /// `surfaceArray444`: an in-place surface converted into the RGBA64F texture.
    pub(super) surface_array_target: ID3D11PixelShader,
    pub(super) constants: ID3D11Buffer,
}

/// A 4:4:4 surface ready to be drawn: read in place through `view` (a slice of the decoder's
/// texture array), or copied into the pass's `SurfaceCopy`. `parameters` are the surface
/// entries' (`surface_parameters`).
pub(super) struct BoundSurface {
    pub(super) view: Option<ID3D11ShaderResourceView>,
    pub(super) parameters: [u32; 8],
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
            // SAFETY: caller guarantees `swap_chain` is the live `IDXGISwapChain` of the renderer.
            unsafe {
                let chain = IDXGISwapChain::from_raw_borrowed(&swap_chain).ok_or("the renderer has no swap chain")?;
                let buffer = chain.GetBuffer::<ID3D11Texture2D>(0).map_err(|e| format!("the back buffer: {e}"))?;
                let device = buffer.GetDevice().map_err(|e| format!("the back buffer's device: {e}"))?;
                let mut target = None;
                device.CreateRenderTargetView(&buffer, None, Some(&mut target)).map_err(|e| format!("the back buffer's view: {e}"))?;
                self.view = Some((swap_chain as usize, size, target.ok_or("no back buffer view")?));
            }
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

/// The source of the plane pass with the 4:4:4 surface entry (`surface444.hlsl`) appended.
pub(super) fn surface_source() -> Vec<u8> {
    [include_bytes!("../../pyrowave.hlsl").as_slice(), b"\n", include_bytes!("../../surface444.hlsl")].concat()
}

/// The source of the direct draw: the plane shader with the surface entry and the direct entries
/// (`planes_direct.hlsl`) appended.
pub(super) fn direct_source() -> Vec<u8> {
    [surface_source().as_slice(), b"\n", include_bytes!("../../planes_direct.hlsl")].concat()
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
pub(super) struct SurfaceCopy {
    pub(super) format: DXGI_FORMAT,
    pub(super) size: (u32, u32),
    pub(super) texture: ID3D11Texture2D,
    pub(super) view: [Option<ID3D11ShaderResourceView>; 1],
}

impl SurfaceCopy {
    pub(super) unsafe fn new(device: &ID3D11Device, format: DXGI_FORMAT, view_format: DXGI_FORMAT, size: (u32, u32)) -> windows::core::Result<Self> {
        // SAFETY: caller guarantees `device` is live on this thread.
        unsafe {
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
}

/// The textures a planar picture is uploaded into, kept while pictures keep their shape.
pub(super) struct FramePlanes {
    pub(super) layout: media::PlanarLayout,
    pub(super) textures: [ID3D11Texture2D; 3],
    pub(super) views: [Option<ID3D11ShaderResourceView>; 3],
}

impl FramePlanes {
    pub(super) unsafe fn new(device: &ID3D11Device, layout: media::PlanarLayout) -> windows::core::Result<Self> {
        let format = if layout.ten_bit { DXGI_FORMAT_R16_UNORM } else { DXGI_FORMAT_R8_UNORM };
        let mut textures = Vec::with_capacity(3);
        let mut views = [None, None, None];
        // SAFETY: caller guarantees `device` is live on this thread.
        unsafe {
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
