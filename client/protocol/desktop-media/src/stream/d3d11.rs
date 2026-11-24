//! Direct3D 11 shader compilation and video presentation passes (`Filter`, `PlanesPass`, `Packed`).

use std::ffi::c_void;

use windows::core::{s, Interface, PCSTR};
use windows::Win32::Graphics::Direct3D::{Fxc::*, ID3DBlob, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

use crate::media;
use super::gpu_planes::{Lease, Pin, Planes};
use super::pyrowave_sdk::fence_ready_within;

mod filter;
mod packed;
mod resources;
mod surface_cache;

pub use filter::Filter;
pub use packed::Packed;
pub use resources::BackBuffer;
pub(crate) use resources::{after_slice_draw, hand_back, surface_parameters, surface_view};

/// One pass in [`Filter::draw_batch`]: `(binding, parameters, area)`.
pub type FilterStep = (usize, [u32; 8], Option<(u32, u32)>);

#[cfg(test)]
use packed::PRESENTATION_BYTES;
use resources::{direct_source, surface_source, BoundSurface, DirectShaders, FramePlanes, SurfaceCopy};
use surface_cache::SurfaceCache;

unsafe fn create_dynamic_cb(device: &ID3D11Device, byte_width: u32) -> windows::core::Result<ID3D11Buffer> {
    let mut buffer = None;
    // SAFETY: Caller guarantees `device` is a live D3D11 device.
    unsafe {
        device.CreateBuffer(&D3D11_BUFFER_DESC {
            ByteWidth: byte_width, Usage: D3D11_USAGE_DYNAMIC,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32, CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
            ..Default::default()
        }, None, Some(&mut buffer))?;
    }
    Ok(buffer.unwrap())
}

unsafe fn write_cb<T>(context: &ID3D11DeviceContext, buffer: &ID3D11Buffer, value: &T) -> windows::core::Result<()> {
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    // SAFETY: Caller guarantees `buffer` is a dynamic CB of at least `size_of::<T>()` bytes on `context`'s device.
    unsafe {
        context.Map(buffer, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped))?;
        std::ptr::copy_nonoverlapping(std::ptr::from_ref(value).cast::<u8>(), mapped.pData.cast::<u8>(), std::mem::size_of::<T>());
        context.Unmap(buffer, 0);
    }
    Ok(())
}

/// Converts the GPU PyroWave decoder's planes to linear RGB in the renderer's target texture.
pub struct PlanesPass {
    context: ID3D11DeviceContext,
    context1: Option<ID3D11DeviceContext1>,
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
    /// Ring caches for decoder texture descriptions and slice SRVs.
    surface_cache: SurfaceCache,
    /// How often `check_surface` asked a texture (a test counts it).
    #[cfg(test)]
    surface_queries: u32,
    /// Frame Warp's HUD mask, and its view for the next `draw_direct` (`t6`, `warp_row`).
    mask: super::frame_warp::D3d11Mask,
    mask_view: Option<ID3D11ShaderResourceView>,
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

impl PlanesPass {
    /// `target` is the `ID3D11Texture2D` of the renderer's RGBA64F picture texture.
    ///
    /// # Safety
    /// `target` must be a live `ID3D11Texture2D` pointer on the renderer's device.
    pub unsafe fn new(target: *mut c_void) -> windows::core::Result<Self> {
        // SAFETY: caller guarantees `target` is a live `ID3D11Texture2D` on the renderer's device.
        unsafe {
            let target = ID3D11Texture2D::from_raw_borrowed(&target).ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_POINTER))?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            target.GetDesc(&mut desc);
            let device = target.GetDevice()?;
            let context = device.GetImmediateContext()?;
            let context1 = context.cast::<ID3D11DeviceContext1>().ok();
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
            let constants = Some(create_dynamic_cb(&device, 32)?);
            let mut sampler = None;
            device.CreateSamplerState(&D3D11_SAMPLER_DESC { Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP, AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP, MaxLOD: f32::MAX, ..Default::default() }, Some(&mut sampler))?;
            let mut raster = None;
            device.CreateRasterizerState(&D3D11_RASTERIZER_DESC { FillMode: D3D11_FILL_SOLID, CullMode: D3D11_CULL_NONE,
                DepthClipEnable: true.into(), ScissorEnable: true.into(), ..Default::default() }, Some(&mut raster))?;
            Ok(Self { context, context1, queue, deferred: deferred.unwrap(), device, target: [target_view],
                vertex: vertex.unwrap(), fragment: fragment.unwrap(), constants: [constants], sampler: [sampler],
                raster: raster.unwrap(), size: (desc.Width, desc.Height), views: Default::default(), owner: None, frame: None,
                surface: None, surface_fragment: None, direct: None, back_buffer: BackBuffer::default(), bound: None, surface_cache: SurfaceCache::default(), #[cfg(test)] surface_queries: 0,
                mask: Default::default(), mask_view: None })
        }
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
            // SAFETY: A dynamic constant buffer on the pass's device.
            let constants = unsafe { create_dynamic_cb(&self.device, std::mem::size_of::<super::direct_planes::Constants>() as u32) }
                .map_err(|e| format!("the direct plane constants cannot be made: {e}"))?;
            self.direct = Some(DirectShaders { planes, surface, surface_array, surface_array_target, constants });
        }
        Ok(self.direct.as_ref().expect("made above"))
    }

    /// Lets go of what the pass keeps of the decoder's textures (the bound surface, its slice views
    /// and the texture facts), for a picture that is not a decoder surface: the decoder that made
    /// them may be gone, and they would keep a pool slice and its device alive.
    pub fn release_surface(&mut self) {
        self.bound = None;
        self.surface_cache.clear();
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
        // SAFETY: caller guarantees `swap_chain` is the live swap chain of the renderer.
        unsafe { self.back_buffer.view(swap_chain, size) }
    }

    /// Call before SDL runs its queue: a view for another swap chain or size goes.
    pub fn keep_back_buffer_if(&mut self, swap_chain: usize, size: (i32, i32)) {
        self.back_buffer.keep_if(swap_chain, size);
    }

    /// Draws a 4:4:4 D3D11VA picture (`media::FRAME_D3D11_444`: AYUV, Y410 or Y416) into the
    /// target, converted as a planar picture is: its slice is copied under FFmpeg's device lock
    /// into a texture of the pass, which the surface entry reads.
    pub fn draw_surface(&mut self, frame: &media::Frame, info: &media::FrameInfo, parameters: &[u32; 8]) -> Result<(), String> {
        frame.wait_d3d11_ready()?;
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
        // SAFETY: Both textures are live on this device, the region covers `copy.texture`, and the
        // immediate context is used under the lock FFmpeg holds while it decodes.
        lock(&mut || unsafe {
            if let Some(c1) = &self.context1 {
                c1.CopySubresourceRegion1(&copy.texture, 0, 0, 0, 0, source, subresource, Some(&region), D3D11_COPY_DISCARD.0 as u32);
            } else {
                self.context.CopySubresourceRegion(&copy.texture, 0, 0, 0, 0, source, subresource, Some(&region));
            }
        });
        Ok(parameters)
    }

    /// What `draw_texture` checks before it reads slice `slice` of `source`: a format this pass
    /// reads, a picture that fits, the pass's own device. The texture's description and the
    /// constants of the surface entries.
    fn check_surface(&mut self, source: &ID3D11Texture2D, slice: u32, size: (u32, u32), parameters: &[u32; 8]) -> Result<(D3D11_TEXTURE2D_DESC, [u32; 8]), String> {
        let desc = self.surface_cache.desc(
            &self.device,
            source,
            #[cfg(test)]
            &mut self.surface_queries,
        )?;
        let Some(parameters) = surface_parameters(*parameters, desc.Format) else {
            return Err(format!("the decoder's pictures are DXGI format {}, not a 4:4:4 format this pass reads", desc.Format.0));
        };
        if size.0 == 0 || size.1 == 0 || size.0 > desc.Width || size.1 > desc.Height || slice >= desc.ArraySize {
            return Err(format!("a {}x{} picture in slice {slice} does not fit the decoder's {}x{}x{} textures", size.0, size.1, desc.Width, desc.Height, desc.ArraySize));
        }
        Ok((desc, parameters))
    }

    /// Readies a 4:4:4 D3D11VA picture for `draw_direct` (`DirectSource::Surface`) and
    /// `draw_bound_surface`. A readable decoder texture is read in place (the caller holds the
    /// frame and draws under its lock): true. Otherwise, or when no view of it can be made, its
    /// slice is copied now into the pass's texture (under the decoder's lock): false.
    pub fn bind_surface(&mut self, frame: &media::Frame, info: &media::FrameInfo, parameters: &[u32; 8], in_place: bool) -> Result<bool, String> {
        frame.wait_d3d11_ready()?;
        let surface = frame.d3d11_surface().ok_or("the picture is not a Direct3D 11 decoder surface")?;
        let raw = surface.texture;
        // SAFETY: The live frame's texture array, borrowed while the frame is.
        let source = unsafe { ID3D11Texture2D::from_raw_borrowed(&raw) }.ok_or("the picture has no texture")?;
        let size = (info.width.max(0) as u32, info.height.max(0) as u32);
        if in_place && surface.readable {
            let (desc, parameters) = self.check_surface(source, surface.slice, size, parameters)?;
            match self.slice_view(source, raw as usize, surface.slice, desc.Format, &mut |op: &mut dyn FnMut()| frame.with_shared_device_lock(op)) {
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

    /// A view of slice `slice` of `source`, creating it under `lock` (FFmpeg's decoder lock).
    fn slice_view(&mut self, source: &ID3D11Texture2D, key: usize, slice: u32, format: DXGI_FORMAT,
        lock: &mut dyn FnMut(&mut dyn FnMut())) -> Result<ID3D11ShaderResourceView, String> {
        self.surface_cache.slice_view(&self.device, source, key, slice, format, lock)
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
            write_cb(c, self.constants[0].as_ref().unwrap(), parameters)?;
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
        let (fragment, slot, views, count, parameters): (ID3D11PixelShader, u32, [Option<ID3D11ShaderResourceView>; 3], usize, [u32; 8]) = match source {
            DirectSource::Lease { lease, fence_value, deadline } => {
                let planes = lease.planes().clone();
                self.retarget(&planes);
                fence_ready_within(planes.decode_fence(), *fence_value, *deadline)?;
                let views = self.views(&planes, lease.set()).map_err(|e| format!("cannot read the PyroWave planes: {e}"))?;
                // SAFETY: The immediate context belongs to this (the presenting) thread.
                unsafe { self.queue.Wait(planes.decode_fence(), *fence_value) }.map_err(|e| format!("cannot order the picture after the PyroWave decode: {e}"))?;
                (planes_entry, 1, views, 3, *parameters)
            }
            DirectSource::Frame => {
                let frame = self.frame.as_ref().ok_or("no planar picture was uploaded")?;
                (planes_entry, 1, frame.views.clone(), 3, *parameters)
            }
            DirectSource::Surface => {
                let bound = self.bound.as_ref().ok_or("no 4:4:4 picture is bound")?;
                match &bound.view {
                    Some(view) => (array_entry, 5, [Some(view.clone()), None, None], 1, bound.parameters),
                    None => (surface_entry, 4, [self.surface.as_ref().ok_or("no 4:4:4 picture was copied")?.view[0].clone(), None, None], 1, bound.parameters),
                }
            }
        };
        // SAFETY: The immediate context belongs to this (the presenting) thread; the deferred
        // context records the draw so SDL's cached immediate-context state is restored.
        unsafe {
            let c = &self.deferred;
            c.ClearState();
            write_cb(c, self.constants[0].as_ref().unwrap(), &parameters).map_err(|e| format!("cannot write plane constants: {e}"))?;
            write_cb(c, &buffer, constants).map_err(|e| format!("cannot write direct plane constants: {e}"))?;
            c.OMSetRenderTargets(Some(&[Some(target.clone())]), None);
            c.RSSetViewports(Some(&[D3D11_VIEWPORT { TopLeftX: viewport[0], TopLeftY: viewport[1], Width: viewport[2], Height: viewport[3], MinDepth: 0.0, MaxDepth: 1.0 }]));
            c.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            c.VSSetShader(&self.vertex, None);
            c.PSSetShader(&fragment, None);
            c.PSSetShaderResources(slot, Some(&views[..count]));
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

/// Compiles `source`, with Frame Warp's shared HLSL spliced in where it includes it
/// (`frame_warp::hlsl`).
///
/// # Safety
/// `entry` and `profile` must be valid NUL-terminated `PCSTR` strings.
pub(crate) unsafe fn compile(source: &[u8], entry: PCSTR, profile: PCSTR) -> windows::core::Result<ID3DBlob> {
    let source = super::frame_warp::hlsl(source);
    let mut code = None;
    let mut errors: Option<ID3DBlob> = None;
    // SAFETY: `source` is in memory; `entry` and `profile` are valid NUL-terminated `PCSTR`s.
    unsafe {
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
        if result.is_err()
            && let Some(errors) = errors
        {
            eprintln!("nightjar shader: {}", String::from_utf8_lossy(std::slice::from_raw_parts(errors.GetBufferPointer().cast(), errors.GetBufferSize())));
        }
        result?;
        Ok(code.unwrap())
    }
}

#[cfg(test)]
mod tests;
