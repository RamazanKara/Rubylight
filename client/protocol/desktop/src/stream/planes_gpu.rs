//! Draws the GPU decoder's planes through SDL's GPU renderer (brief G3, Linux).
//!
//! The planes the SDK decoded sit in images on the renderer's own device (`zero_copy.rs`) or in
//! CPU buffers (`gpu_planes::Lease`). This pass copies them into three persistent R8 or R16 UNORM
//! textures of the renderer's device, on the GPU from the images (wrapped as SDL textures once per
//! plane set) or as an upload from the buffers, and draws a full-target rectangle with
//! `pyrowave-planes.spv`, the `planes` entry of pyrowave.hlsl, which converts the planes to the
//! same linear RGB as the CPU path. Nothing here waits for the GPU: the copies and the draw are
//! ordered by the renderer's own queue, and a plane set copied on the GPU goes back to the decoder
//! with the reuse value signalled behind the copy.

use std::ptr;
use std::sync::Arc;
use std::time::Instant;

use sdl3_sys::everything::*;

use super::gpu_planes::{Lease, Planes};
use super::pyrowave_sdk::zero_copy::{SharedImages, TEXTURE_IMAGE_PROPERTY};
use super::pyrowave_sdk::GpuConfig;
use super::stage_times::Stages;

const PLANES_SPIRV: &[u8] = include_bytes!("../pyrowave-planes.spv");

fn device_of(renderer: *mut SDL_Renderer) -> *mut SDL_GPUDevice {
    crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_GPU_DEVICE_POINTER).cast()
}

fn plane_format(ten_bit: bool) -> SDL_GPUTextureFormat {
    if ten_bit { SDL_GPU_TEXTUREFORMAT_R16_UNORM } else { SDL_GPU_TEXTUREFORMAT_R8_UNORM }
}

/// # Safety
/// `device` must be a live GPU device of the calling thread's renderer.
unsafe fn create_shader(device: *mut SDL_GPUDevice) -> *mut SDL_GPUShader {
    SDL_CreateGPUShader(device, &SDL_GPUShaderCreateInfo {
        code: PLANES_SPIRV.as_ptr(), code_size: PLANES_SPIRV.len(), entrypoint: c"planes".as_ptr(),
        format: SDL_GPU_SHADERFORMAT_SPIRV, stage: SDL_GPU_SHADERSTAGE_FRAGMENT,
        num_samplers: 3, num_uniform_buffers: 1, ..Default::default() })
}

pub struct PlanesPass {
    device: *mut SDL_GPUDevice,
    shader: *mut SDL_GPUShader,
    sampler: *mut SDL_GPUSampler,
    textures: [*mut SDL_GPUTexture; 3],
    transfer: *mut SDL_GPUTransferBuffer,
    state: *mut SDL_GPURenderState,
    /// The width and height of each plane texture.
    sizes: [(u32, u32); 3],
    sample_bytes: usize,
    /// The decoder's images wrapped as textures of this device, per plane set, for the planes
    /// they belong to.
    wrapped: Option<(Arc<Planes>, Vec<[*mut SDL_GPUTexture; 3]>)>,
}

impl PlanesPass {
    /// Whether the plane shader can be built on `device`, for a probe.
    ///
    /// # Safety
    /// `device` must be the live `SDL_GPUDevice` of a renderer, used on its own thread.
    pub unsafe fn compiles(device: *mut SDL_GPUDevice, ten_bit: bool) -> Result<(), String> {
        if device.is_null() {
            return Err("PyroWave GPU decoding needs the SDL GPU renderer".to_owned());
        }
        if !SDL_GPUTextureSupportsFormat(device, plane_format(ten_bit), SDL_GPU_TEXTURETYPE_2D, SDL_GPU_TEXTUREUSAGE_SAMPLER) {
            return Err(format!("the GPU renderer cannot sample {} planes", if ten_bit { "R16" } else { "R8" }));
        }
        let shader = create_shader(device);
        if shader.is_null() {
            return Err(format!("the PyroWave plane shader does not load: {}", crate::sdl::error()));
        }
        SDL_ReleaseGPUShader(device, shader);
        Ok(())
    }

    /// The pass for planes shaped like `config`, drawing on `renderer`'s GPU device.
    ///
    /// # Safety
    /// `renderer` must be the live SDL GPU renderer of the calling thread.
    pub unsafe fn new(renderer: *mut SDL_Renderer, config: GpuConfig) -> Result<Self, String> {
        Self::with_planes(renderer, [0, 1, 2].map(|plane| config.plane_size(plane)), config.ten_bit)
    }

    /// The pass for software-decoded planar pictures shaped like `layout`.
    ///
    /// # Safety
    /// `renderer` must be the live SDL GPU renderer of the calling thread.
    pub unsafe fn for_frames(renderer: *mut SDL_Renderer, layout: crate::media::PlanarLayout) -> Result<Self, String> {
        Self::with_planes(renderer, [0, 1, 2].map(|plane| layout.plane_size(plane)), layout.ten_bit)
    }

    /// The pass for Vulkan video pictures: it draws `textures`, the luma and the interleaved chroma
    /// of `vulkan_video::Planes`, which the presenter copies each picture into on the GPU, with
    /// flag 256 (`draw_bound`); it has no textures or upload of its own.
    ///
    /// # Safety
    /// `renderer` must be the live SDL GPU renderer of the calling thread, and `textures` live
    /// textures of its device that outlive the pass.
    pub unsafe fn for_vulkan(renderer: *mut SDL_Renderer, textures: [*mut SDL_GPUTexture; 2]) -> Result<Self, String> {
        let mut pass = Self::shader_only(renderer, [(0, 0); 3], 1)?;
        pass.bind(renderer, [textures[0], textures[1], textures[1]])?;
        Ok(pass)
    }

    /// Draws the textures a `for_vulkan` pass was made with, converted, over all of `target`
    /// (null: nothing yet, `draw_direct` draws them later).
    ///
    /// # Safety
    /// `renderer` and `target` must be live and the pass the one made on this renderer.
    pub unsafe fn draw_bound(&mut self, renderer: *mut SDL_Renderer, target: *mut SDL_Texture, parameters: &[u32; 8]) -> Result<(), String> {
        self.convert(renderer, target, parameters, None)
    }

    unsafe fn with_planes(renderer: *mut SDL_Renderer, sizes: [(u32, u32); 3], ten_bit: bool) -> Result<Self, String> {
        let mut pass = Self::shader_only(renderer, sizes, if ten_bit { 2 } else { 1 })?;
        let device = pass.device;
        for (plane, texture) in pass.textures.iter_mut().enumerate() {
            let (width, height) = sizes[plane];
            *texture = SDL_CreateGPUTexture(device, &SDL_GPUTextureCreateInfo {
                r#type: SDL_GPU_TEXTURETYPE_2D, format: plane_format(ten_bit), usage: SDL_GPU_TEXTUREUSAGE_SAMPLER,
                width, height, layer_count_or_depth: 1, num_levels: 1, sample_count: SDL_GPU_SAMPLECOUNT_1, ..Default::default() });
            if texture.is_null() {
                return Err(format!("a {width}x{height} plane texture cannot be created: {}", crate::sdl::error()));
            }
        }
        let bytes: usize = (0..3).map(|plane| pass.plane_bytes(plane)).sum();
        pass.transfer = SDL_CreateGPUTransferBuffer(device, &SDL_GPUTransferBufferCreateInfo {
            usage: SDL_GPU_TRANSFERBUFFERUSAGE_UPLOAD, size: bytes as u32, ..Default::default() });
        if pass.transfer.is_null() {
            return Err(format!("the plane transfer buffer cannot be created: {}", crate::sdl::error()));
        }
        pass.bind(renderer, pass.textures)?;
        Ok(pass)
    }

    /// The pass's shader and sampler, without textures or a render state yet.
    unsafe fn shader_only(renderer: *mut SDL_Renderer, sizes: [(u32, u32); 3], sample_bytes: usize) -> Result<Self, String> {
        let device = device_of(renderer);
        if device.is_null() {
            return Err("it needs the SDL GPU renderer".into());
        }
        let mut pass = Self {
            device, shader: ptr::null_mut(), sampler: ptr::null_mut(), textures: [ptr::null_mut(); 3],
            transfer: ptr::null_mut(), state: ptr::null_mut(), sizes, sample_bytes, wrapped: None,
        };
        pass.shader = create_shader(device);
        if pass.shader.is_null() {
            return Err(format!("the plane shader does not load: {}", crate::sdl::error()));
        }
        pass.sampler = SDL_CreateGPUSampler(device, &SDL_GPUSamplerCreateInfo {
            min_filter: SDL_GPU_FILTER_LINEAR, mag_filter: SDL_GPU_FILTER_LINEAR, mipmap_mode: SDL_GPU_SAMPLERMIPMAPMODE_NEAREST,
            address_mode_u: SDL_GPU_SAMPLERADDRESSMODE_CLAMP_TO_EDGE, address_mode_v: SDL_GPU_SAMPLERADDRESSMODE_CLAMP_TO_EDGE,
            address_mode_w: SDL_GPU_SAMPLERADDRESSMODE_CLAMP_TO_EDGE, ..Default::default() });
        if pass.sampler.is_null() {
            return Err(format!("the plane sampler cannot be created: {}", crate::sdl::error()));
        }
        Ok(pass)
    }

    /// The render state drawing `textures` in the shader's three plane slots.
    unsafe fn bind(&mut self, renderer: *mut SDL_Renderer, textures: [*mut SDL_GPUTexture; 3]) -> Result<(), String> {
        let bindings = textures.map(|texture| SDL_GPUTextureSamplerBinding { texture, sampler: self.sampler });
        self.state = SDL_CreateGPURenderState(renderer, &SDL_GPURenderStateCreateInfo {
            fragment_shader: self.shader, num_sampler_bindings: 3, sampler_bindings: bindings.as_ptr(), ..Default::default() });
        if self.state.is_null() {
            return Err(format!("the plane render state cannot be created: {}", crate::sdl::error()));
        }
        Ok(())
    }

    /// Copies the planes of `lease` (on the GPU, or as an upload) and draws them, converted, over
    /// all of `target` (the renderer's RGBA64F picture texture). `parameters` are the shader's
    /// constants. The CPU time of filling and submitting the copy goes to `stages`' upload, that
    /// of the conversion to its passes (the GPU runs on after the submit returns).
    ///
    /// # Safety
    /// `renderer` and `target` must be live and the pass the one made on this renderer.
    pub unsafe fn draw(&mut self, renderer: *mut SDL_Renderer, target: *mut SDL_Texture, lease: &Lease, parameters: &[u32; 8], stages: &mut Stages<2>) -> Result<(), String> {
        if let Some(images) = lease.planes().images() {
            let images = images.clone();
            return self.draw_shared(renderer, target, lease, &images, parameters, stages);
        }
        let bytes = [0, 1, 2].map(|plane| self.plane_bytes(plane));
        self.draw_with(renderer, target, parameters, Some(stages), |mapped, offsets| {
            lease.with_buffers(|buffers| {
                for (plane, buffer) in buffers.iter().enumerate() {
                    ptr::copy_nonoverlapping(buffer.as_ptr(), mapped.add(offsets[plane]), bytes[plane]);
                }
            });
        })
    }

    /// `draw` for a set in the decoder's images: copies it into the plane textures on the GPU,
    /// signals the reuse timeline behind the copy and gives the set back with that value, so the
    /// decoder can write it again as soon as the copy is done, not after the draw.
    unsafe fn draw_shared(&mut self, renderer: *mut SDL_Renderer, target: *mut SDL_Texture, lease: &Lease, images: &SharedImages, parameters: &[u32; 8], stages: &mut Stages<2>) -> Result<(), String> {
        let step = Instant::now();
        if !SDL_FlushRenderer(renderer) {
            return Err(format!("the renderer cannot flush: {}", crate::sdl::error()));
        }
        let sources = self.wrap(lease.planes(), images)?[lease.set()];
        let commands = SDL_AcquireGPUCommandBuffer(self.device);
        if commands.is_null() {
            return Err(format!("no GPU command buffer for the plane copy: {}", crate::sdl::error()));
        }
        let copy = SDL_BeginGPUCopyPass(commands);
        for (plane, source) in sources.into_iter().enumerate() {
            let (width, height) = self.sizes[plane];
            SDL_CopyGPUTextureToTexture(copy,
                &SDL_GPUTextureLocation { texture: source, ..Default::default() },
                &SDL_GPUTextureLocation { texture: self.textures[plane], ..Default::default() },
                width, height, 1, true);
        }
        SDL_EndGPUCopyPass(copy);
        if !SDL_SubmitGPUCommandBuffer(commands) {
            return Err(format!("the plane copy cannot be submitted: {}", crate::sdl::error()));
        }
        lease.release_after(images.signal_reuse()?);
        stages.add(super::pyrowave_gpu::UPLOAD, step);
        self.convert(renderer, target, parameters, Some(stages))
    }

    /// The textures wrapping each plane set of `planes`' images, made on first use. Those of
    /// earlier planes (an earlier decoder's) are released first.
    unsafe fn wrap(&mut self, planes: &Arc<Planes>, images: &SharedImages) -> Result<&[[*mut SDL_GPUTexture; 3]], String> {
        if self.wrapped.as_ref().is_some_and(|(wrapped, _)| !Arc::ptr_eq(wrapped, planes)) {
            self.unwrap();
        }
        if self.wrapped.is_none() {
            let config = images.config();
            let mut sets: Vec<[*mut SDL_GPUTexture; 3]> = Vec::new();
            for set in 0..config.slots {
                let mut textures = [ptr::null_mut(); 3];
                for (plane, texture) in textures.iter_mut().enumerate() {
                    let (width, height) = config.plane_size(plane);
                    let props = SDL_CreateProperties();
                    SDL_SetNumberProperty(props, TEXTURE_IMAGE_PROPERTY.as_ptr(), ash::vk::Handle::as_raw(images.image(set, plane)) as i64);
                    // Compute storage, so SDL expects the image in GENERAL, where the decoder leaves it.
                    *texture = SDL_CreateGPUTexture(self.device, &SDL_GPUTextureCreateInfo {
                        r#type: SDL_GPU_TEXTURETYPE_2D, format: plane_format(config.ten_bit), usage: SDL_GPU_TEXTUREUSAGE_COMPUTE_STORAGE_WRITE,
                        width, height, layer_count_or_depth: 1, num_levels: 1, sample_count: SDL_GPU_SAMPLECOUNT_1, props });
                    SDL_DestroyProperties(props);
                }
                // Kept before the check, so `unwrap` releases what was made.
                let failed = textures.iter().any(|texture| texture.is_null());
                sets.push(textures);
                if failed {
                    self.wrapped = Some((planes.clone(), sets));
                    let error = crate::sdl::error();
                    self.unwrap();
                    return Err(format!("the decoder's plane images cannot be wrapped as textures: {error}"));
                }
            }
            self.wrapped = Some((planes.clone(), sets));
        }
        Ok(self.wrapped.as_ref().map_or(&[], |(_, sets)| sets.as_slice()))
    }

    /// Releases the wrapped textures and lets their images go. SDL destroys a texture's views only
    /// once its last command is done, and the images must outlive them, so this waits for that.
    unsafe fn unwrap(&mut self) {
        let Some((planes, sets)) = self.wrapped.take() else { return };
        for texture in sets.into_iter().flatten().filter(|texture| !texture.is_null()) {
            SDL_ReleaseGPUTexture(self.device, texture);
        }
        if !SDL_WaitForGPUIdle(self.device) {
            // The views may still be in use: keep the images rather than free them under the GPU.
            eprintln!("nightjar: the GPU did not go idle after the PyroWave planes ({}), so their images stay allocated", crate::sdl::error());
            std::mem::forget(planes);
        }
    }

    /// Uploads the planes of a software-decoded planar picture (row by row: FFmpeg pads its rows),
    /// or of an NV12 or P010 one (`PlanarLayout::of_semi_planar`), and draws them, converted, over
    /// all of `target`.
    ///
    /// # Safety
    /// `renderer` and `target` must be live, the pass the one made on this renderer for pictures
    /// shaped like `info`, and `info` the description of a live frame.
    pub unsafe fn draw_frame(&mut self, renderer: *mut SDL_Renderer, target: *mut SDL_Texture, info: &crate::media::FrameInfo, parameters: &[u32; 8]) -> Result<(), String> {
        let planes = if semi_planar(info) { 2 } else { 3 };
        if let Some(plane) = (0..planes).find(|&plane| info.data[plane].is_null() || info.linesize[plane] <= 0) {
            return Err(format!("plane {plane} of the picture is missing"));
        }
        let (sizes, sample_bytes) = (self.sizes, self.sample_bytes);
        self.draw_with(renderer, target, parameters, None, |mapped, offsets| fill_planes(info, sizes, sample_bytes, mapped, offsets))
    }

    /// The bytes of plane `plane` in the transfer buffer (tightly packed rows).
    fn plane_bytes(&self, plane: usize) -> usize {
        let (width, height) = self.sizes[plane];
        width as usize * height as usize * self.sample_bytes
    }

    /// Fills the transfer buffer with `fill` (given the mapped buffer and each plane's offset in
    /// it), uploads it into the plane textures and draws them over `target`.
    unsafe fn draw_with(&mut self, renderer: *mut SDL_Renderer, target: *mut SDL_Texture, parameters: &[u32; 8], mut stages: Option<&mut Stages<2>>, fill: impl FnOnce(*mut u8, [usize; 3])) -> Result<(), String> {
        let step = Instant::now();
        if !SDL_FlushRenderer(renderer) {
            return Err(format!("the renderer cannot flush: {}", crate::sdl::error()));
        }
        let mapped = SDL_MapGPUTransferBuffer(self.device, self.transfer, true).cast::<u8>();
        if mapped.is_null() {
            return Err(format!("the plane transfer buffer cannot be mapped: {}", crate::sdl::error()));
        }
        let mut offsets = [0usize; 3];
        for plane in 1..3 {
            offsets[plane] = offsets[plane - 1] + self.plane_bytes(plane - 1);
        }
        fill(mapped, offsets);
        SDL_UnmapGPUTransferBuffer(self.device, self.transfer);
        let commands = SDL_AcquireGPUCommandBuffer(self.device);
        if commands.is_null() {
            return Err(format!("no GPU command buffer for the plane upload: {}", crate::sdl::error()));
        }
        let copy = SDL_BeginGPUCopyPass(commands);
        for (plane, offset) in offsets.into_iter().enumerate() {
            let (width, height) = self.sizes[plane];
            SDL_UploadToGPUTexture(copy,
                &SDL_GPUTextureTransferInfo { transfer_buffer: self.transfer, offset: offset as u32, pixels_per_row: width, rows_per_layer: height },
                &SDL_GPUTextureRegion { texture: self.textures[plane], w: width, h: height, d: 1, ..Default::default() },
                true);
        }
        SDL_EndGPUCopyPass(copy);
        if !SDL_SubmitGPUCommandBuffer(commands) {
            return Err(format!("the plane upload cannot be submitted: {}", crate::sdl::error()));
        }
        if let Some(stages) = stages.as_deref_mut() { stages.add(super::pyrowave_gpu::UPLOAD, step); }
        self.convert(renderer, target, parameters, stages)
    }

    /// Draws the plane textures, converted, over `target`. A null `target` leaves the planes
    /// copied and undrawn, for `draw_direct` (or a later `draw_bound`) to draw.
    unsafe fn convert(&mut self, renderer: *mut SDL_Renderer, target: *mut SDL_Texture, parameters: &[u32; 8], stages: Option<&mut Stages<2>>) -> Result<(), String> {
        if target.is_null() {
            return Ok(());
        }
        let step = Instant::now();
        self.set_uniforms(&Uniforms { parameters: *parameters, ..Default::default() })?;
        let previous = SDL_GetRenderTarget(renderer);
        if !SDL_SetRenderTarget(renderer, target) {
            return Err(format!("the picture texture cannot be the render target: {}", crate::sdl::error()));
        }
        let drawn = SDL_SetGPURenderState(renderer, self.state) && SDL_RenderFillRect(renderer, ptr::null()) && SDL_FlushRenderer(renderer);
        let error = if drawn { String::new() } else { crate::sdl::error() };
        SDL_SetGPURenderState(renderer, ptr::null_mut());
        SDL_SetRenderTarget(renderer, previous);
        if let Some(stages) = stages { stages.add(super::pyrowave_gpu::PASSES, step); }
        if drawn { Ok(()) } else { Err(format!("the planes cannot be drawn: {error}")) }
    }
}

impl PlanesPass {
    unsafe fn set_uniforms(&mut self, uniforms: &Uniforms) -> Result<(), String> {
        if !SDL_SetGPURenderStateFragmentUniforms(self.state, 0, (uniforms as *const Uniforms).cast(), std::mem::size_of::<Uniforms>() as u32) {
            return Err(format!("the plane constants cannot be set: {}", crate::sdl::error()));
        }
        Ok(())
    }

    /// Draws the copied planes, converted and scaled, into `destination` of the renderer's current
    /// target (the back buffer): the picture texture's pass and SDL's draw of it in one. `visible`
    /// is the picture's size in the planes; `look` is what SDL's draw would apply (`SdlLook`).
    /// Converting and scaling in the one draw that reaches the swapchain follows moonlight-qt's
    /// and chiaki-ng's Vulkan renderers.
    ///
    /// # Safety
    /// `renderer` must be live and the pass the one made on it, its planes copied for `parameters`.
    pub unsafe fn draw_direct(&mut self, renderer: *mut SDL_Renderer, parameters: &[u32; 8], destination: &SDL_FRect, visible: (i32, i32), look: SdlLook) -> Result<(), String> {
        let mut viewport = SDL_Rect::default();
        SDL_GetRenderViewport(renderer, &mut viewport);
        self.set_uniforms(&direct_uniforms(parameters, destination, (viewport.x, viewport.y), visible, look))?;
        let drawn = SDL_SetGPURenderState(renderer, self.state) && SDL_RenderFillRect(renderer, destination);
        let error = if drawn { String::new() } else { crate::sdl::error() };
        SDL_SetGPURenderState(renderer, ptr::null_mut());
        if drawn { Ok(()) } else { Err(format!("the planes cannot be drawn into the back buffer: {error}")) }
    }
}

/// The plane shader's constants (`Picture` in pyrowave.hlsl, Vulkan build): the pass parameters,
/// then what flag 512 reads.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Uniforms {
    pub parameters: [u32; 8],
    pub destination: [f32; 4],
    pub picture: [f32; 4],
    pub output_scale: f32,
    pub tonemap: [f32; 2],
    pub unused: f32,
}

/// The shader's flag for a draw straight into the target rectangle.
pub const DIRECT_FLAG: u32 = 512;

/// What SDL's draw of the RGBA64F linear picture texture into the back buffer does to it
/// (`texture_advanced.frag.hlsl`): its tone map, when the texture's HDR headroom is above the
/// output's, then its colour scale (the output's SDR white times the renderer's colour scale).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SdlLook {
    pub texture_headroom: f32,
    pub output_headroom: f32,
    pub sdr_white: f32,
    pub color_scale: f32,
}

impl SdlLook {
    /// SDL's TONEMAP_CHROME factors (`CalculateAdvancedShaderConstants`), None without a tone map.
    pub fn tonemap(&self) -> Option<(f32, f32)> {
        (self.texture_headroom > self.output_headroom && self.output_headroom > 0.0)
            .then(|| (self.output_headroom / (self.texture_headroom * self.texture_headroom), 1.0 / self.output_headroom))
    }
}

/// The constants of a `draw_direct` into `destination` (render coordinates) of a viewport at `origin`.
pub fn direct_uniforms(parameters: &[u32; 8], destination: &SDL_FRect, origin: (i32, i32), visible: (i32, i32), look: SdlLook) -> Uniforms {
    let mut parameters = *parameters;
    parameters[6] |= DIRECT_FLAG;
    let (tonemap1, tonemap2) = look.tonemap().unwrap_or((0.0, 0.0));
    Uniforms {
        parameters,
        destination: [destination.x + origin.0 as f32, destination.y + origin.1 as f32, destination.w.max(1.0), destination.h.max(1.0)],
        picture: [visible.0 as f32, visible.1 as f32, 0.0, 0.0],
        output_scale: look.sdr_white * look.color_scale,
        tonemap: [tonemap1, tonemap2],
        unused: 0.0,
    }
}

/// Whether a picture's chroma is one interleaved plane (NV12, P010).
fn semi_planar(info: &crate::media::FrameInfo) -> bool {
    matches!(info.kind, crate::media::FRAME_NV12 | crate::media::FRAME_P010)
}

/// Copies a picture's planes into the transfer buffer as the tightly packed planes `sizes` gives.
/// A planar picture's rows go as they are. An NV12 or P010 picture's interleaved chroma is split
/// into its Cb and Cr planes, and P010's samples, in the high 10 bits of each word, move to the low
/// bits, where the shader reads FFmpeg's 10-bit planes (flag 64).
///
/// # Safety
/// `info` describes a live frame with at least `sizes` samples in each plane, and `mapped` has
/// room for the planes at `offsets`.
unsafe fn fill_planes(info: &crate::media::FrameInfo, sizes: [(u32, u32); 3], sample_bytes: usize, mapped: *mut u8, offsets: [usize; 3]) {
    if !semi_planar(info) {
        for (plane, &(width, height)) in sizes.iter().enumerate() {
            let row = width as usize * sample_bytes;
            for y in 0..height as usize {
                ptr::copy_nonoverlapping(info.data[plane].add(y * info.linesize[plane] as usize), mapped.add(offsets[plane] + y * row), row);
            }
        }
        return;
    }
    let p010 = info.kind == crate::media::FRAME_P010;
    let put = |target: &mut [u8], source: &[u8]| {
        if p010 {
            target.copy_from_slice(&(u16::from_le_bytes([source[0], source[1]]) >> 6).to_le_bytes());
        } else {
            target.copy_from_slice(source);
        }
    };
    let (width, height) = sizes[0];
    let row = width as usize * sample_bytes;
    for y in 0..height as usize {
        let source = std::slice::from_raw_parts(info.data[0].add(y * info.linesize[0] as usize), row);
        let target = std::slice::from_raw_parts_mut(mapped.add(offsets[0] + y * row), row);
        if p010 {
            target.chunks_exact_mut(2).zip(source.chunks_exact(2)).for_each(|(target, source)| put(target, source));
        } else {
            target.copy_from_slice(source);
        }
    }
    let (width, height) = sizes[1];
    let row = width as usize * sample_bytes;
    for y in 0..height as usize {
        let source = std::slice::from_raw_parts(info.data[1].add(y * info.linesize[1] as usize), 2 * row);
        let cb = std::slice::from_raw_parts_mut(mapped.add(offsets[1] + y * row), row);
        let cr = std::slice::from_raw_parts_mut(mapped.add(offsets[2] + y * row), row);
        for (pair, (cb, cr)) in source.chunks_exact(2 * sample_bytes).zip(cb.chunks_exact_mut(sample_bytes).zip(cr.chunks_exact_mut(sample_bytes))) {
            put(cb, &pair[..sample_bytes]);
            put(cr, &pair[sample_bytes..]);
        }
    }
}

impl Drop for PlanesPass {
    fn drop(&mut self) {
        // SAFETY: These objects belong to the renderer's device, which outlives the pass.
        unsafe {
            self.unwrap();
            if !self.state.is_null() { SDL_DestroyGPURenderState(self.state); }
            if !self.transfer.is_null() { SDL_ReleaseGPUTransferBuffer(self.device, self.transfer); }
            for texture in self.textures { if !texture.is_null() { SDL_ReleaseGPUTexture(self.device, texture); } }
            if !self.sampler.is_null() { SDL_ReleaseGPUSampler(self.device, self.sampler); }
            if !self.shader.is_null() { SDL_ReleaseGPUShader(self.device, self.shader); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::{FrameInfo, PlanarLayout, FRAME_NV12, FRAME_P010};

    const SDR: SdlLook = SdlLook { texture_headroom: 1.0, output_headroom: 1.0, sdr_white: 1.0, color_scale: 1.0 };

    /// The constants are the shader's 80-byte `Picture` block, in its order.
    #[test]
    fn the_direct_constants_match_the_shader_block() {
        assert_eq!(std::mem::size_of::<Uniforms>(), 80);
        assert_eq!(std::mem::offset_of!(Uniforms, destination), 32);
        assert_eq!(std::mem::offset_of!(Uniforms, picture), 48);
        assert_eq!(std::mem::offset_of!(Uniforms, output_scale), 64);
        assert_eq!(std::mem::offset_of!(Uniforms, tonemap), 68);
    }

    /// The draw into the back buffer flags itself, places the picture in the viewport and keeps
    /// the picture texture's parameters otherwise, so it converts exactly as the texture would.
    #[test]
    fn a_direct_draw_lands_where_sdl_would_draw_the_texture() {
        let parameters = [1920, 1080, 1920, 1080, 0, 3, 0b1_0101, 2.5f32.to_bits()];
        let destination = SDL_FRect { x: 240.0, y: 0.0, w: 1440.0, h: 810.0 };
        let uniforms = direct_uniforms(&parameters, &destination, (0, 135), (1920, 1080), SDR);
        assert_eq!(uniforms.parameters[6], 0b1_0101 | DIRECT_FLAG);
        assert_eq!(uniforms.parameters[..6], parameters[..6]);
        assert_eq!(uniforms.parameters[7], parameters[7]);
        assert_eq!(uniforms.destination, [240.0, 135.0, 1440.0, 810.0]);
        assert_eq!(uniforms.picture, [1920.0, 1080.0, 0.0, 0.0]);
        assert_eq!(uniforms.output_scale, 1.0);
        assert_eq!(uniforms.tonemap, [0.0, 0.0], "an SDR picture on an SDR output is not tone mapped");
    }

    /// SDL scales a linear texture by the output's SDR white and its colour scale, and tone maps
    /// it (TONEMAP_CHROME) only when the texture carries more headroom than an HDR output has.
    #[test]
    fn a_direct_draw_scales_and_tone_maps_as_sdl_does() {
        let hdr = SdlLook { texture_headroom: 12.0, output_headroom: 4.0, sdr_white: 2.5, color_scale: 0.5 };
        let uniforms = direct_uniforms(&[0; 8], &SDL_FRect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 }, (0, 0), (2, 2), hdr);
        assert_eq!(uniforms.output_scale, 1.25);
        assert_eq!(uniforms.tonemap, [4.0 / 144.0, 0.25]);
        assert_eq!(uniforms.destination[2..], [1.0, 1.0], "an empty rectangle must not divide by zero in the shader");
        assert_eq!(SdlLook { output_headroom: 12.0, ..hdr }.tonemap(), None, "equal headroom needs no tone map");
        assert_eq!(SdlLook { output_headroom: 0.0, ..hdr }.tonemap(), None, "SDL skips the tone map without an output headroom");
        assert_eq!(SDR.tonemap(), None);
    }

    fn picture(kind: i32, luma: &mut [u8], chroma: &mut [u8], linesize: i32) -> FrameInfo {
        FrameInfo {
            width: 3, height: 2, kind, ten_bit: i32::from(kind == FRAME_P010), pts: 0,
            data: [luma.as_mut_ptr(), chroma.as_mut_ptr(), ptr::null_mut()],
            linesize: [linesize; 3], color_space: 9, color_range: 1, color_trc: 18,
        }
    }

    /// An HLG P010 picture from a hardware decoder reaches the plane shader as the 10-bit planar
    /// picture it reads (SDL's P010 texture read it as PQ): Cb and Cr in planes of their own,
    /// every sample in the low 10 bits. Row padding is skipped.
    #[test]
    fn a_p010_picture_is_uploaded_as_ten_bit_planes() {
        // 3x2 luma, 2x1 chroma pairs; rows of 4 words (8 bytes), the last word padding.
        let words = |w: [u16; 8]| w.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>();
        let mut luma = words([64 << 6, 512 << 6, 940 << 6, 0xffff, 1 << 6, 2 << 6, 1023 << 6, 0xffff]);
        let mut chroma = words([100 << 6, 900 << 6, 512 << 6, 513 << 6, 0xffff, 0xffff, 0xffff, 0xffff]);
        let info = picture(FRAME_P010, &mut luma, &mut chroma, 8);
        let layout = PlanarLayout::of_semi_planar(&info).unwrap();
        assert_eq!(layout, PlanarLayout { width: 3, height: 2, chroma444: false, ten_bit: true });
        let sizes = [0, 1, 2].map(|plane| layout.plane_size(plane));
        let mut buffer = vec![0xaau8; (6 + 2 + 2) * 2];
        // SAFETY: the planes above hold `sizes` samples; the buffer holds the three planes.
        unsafe { fill_planes(&info, sizes, 2, buffer.as_mut_ptr(), [0, 12, 16]) };
        let samples: Vec<u16> = buffer.chunks_exact(2).map(|s| u16::from_le_bytes([s[0], s[1]])).collect();
        assert_eq!(samples, [64, 512, 940, 1, 2, 1023, 100, 512, 900, 513]);
    }

    #[test]
    fn an_nv12_picture_is_uploaded_as_eight_bit_planes() {
        let mut luma = vec![16, 128, 235, 0xff, 17, 18, 19, 0xff];
        let mut chroma = vec![60, 200, 128, 129, 0xff, 0xff, 0xff, 0xff];
        let info = picture(FRAME_NV12, &mut luma, &mut chroma, 4);
        let layout = PlanarLayout::of_semi_planar(&info).unwrap();
        assert!(!layout.ten_bit && !layout.chroma444);
        let mut buffer = vec![0xaau8; 6 + 2 + 2];
        // SAFETY: as above.
        unsafe { fill_planes(&info, [0, 1, 2].map(|plane| layout.plane_size(plane)), 1, buffer.as_mut_ptr(), [0, 6, 8]) };
        assert_eq!(buffer, [16, 128, 235, 17, 18, 19, 60, 128, 200, 129]);
    }

    /// The HLG picture on the GPU (a Vulkan device and SDL's offscreen video driver, so ignored:
    /// `cargo test -p nightjar-desktop -- --ignored stream::planes_gpu`): HLG's reference white
    /// (75 %) in P010 and NV12 comes out at BT.2408's 203 nits on a 1000-nit display, and at SDR
    /// white on an SDR one. Read as PQ, the same code would be 983 nits.
    #[test]
    #[ignore = "needs a Vulkan device and SDL's offscreen video driver"]
    fn hlg_reference_white_is_drawn_at_203_nits_from_p010_and_nv12() {
        let (width, height) = (16usize, 8usize);
        let p010_luma = (0..width * height).flat_map(|_| (721u16 << 6).to_le_bytes()).collect::<Vec<u8>>();
        let p010_chroma = (0..width * height / 2).flat_map(|_| (512u16 << 6).to_le_bytes()).collect::<Vec<u8>>();
        let nv12_luma = vec![180u8; width * height];
        let nv12_chroma = vec![128u8; width * height / 2];
        // SAFETY: Plain SDL calls on objects made here, on one thread.
        unsafe {
            SDL_SetHint(SDL_HINT_VIDEO_DRIVER, c"offscreen".as_ptr());
            assert!(SDL_Init(SDL_INIT_VIDEO), "{}", crate::sdl::error());
            let window = SDL_CreateWindow(c"HLG planes".as_ptr(), 64, 64, SDL_WINDOW_HIDDEN);
            let renderer = crate::sdl::create_renderer(window, true);
            assert!(!renderer.is_null(), "{}", crate::sdl::error());
            assert_eq!(crate::sdl::renderer_name(renderer), "gpu");
            let target = SDL_CreateTexture(renderer, SDL_PIXELFORMAT_RGBA64_FLOAT, SDL_TEXTUREACCESS_TARGET, width as i32, height as i32);
            assert!(!target.is_null(), "{}", crate::sdl::error());
            // A 1000-nit HDR output (headroom 12.5 over 80 nits), where 1.0 is 80 nits, and an SDR
            // one, drawn for the reference display with its 203-nit white at 1.0.
            let outputs = [(12.5, 80.0), (1.0, 203.0)];
            for ((kind, luma, chroma, bytes), (headroom, unit)) in [(FRAME_P010, &p010_luma, &p010_chroma, 2), (FRAME_NV12, &nv12_luma, &nv12_chroma, 1)]
                .into_iter().flat_map(|picture| outputs.map(|output| (picture, output)))
            {
                let (mut luma, mut chroma) = (luma.clone(), chroma.clone());
                let mut info = picture(kind, &mut luma, &mut chroma, (width * bytes) as i32);
                (info.width, info.height) = (width as i32, height as i32);
                assert!(super::super::present::hlg_through_planes(&info));
                let layout = PlanarLayout::of_semi_planar(&info).unwrap();
                let parameters = super::super::present::planar_parameters(&info, layout, true, 1.0, headroom);
                let mut pass = PlanesPass::for_frames(renderer, layout).unwrap();
                pass.draw_frame(renderer, target, &info, &parameters).unwrap();
                assert!(SDL_SetRenderTarget(renderer, target), "{}", crate::sdl::error());
                let readback = SDL_RenderReadPixels(renderer, ptr::null());
                assert!(!readback.is_null(), "{}", crate::sdl::error());
                let pixels = SDL_ConvertSurface(readback, SDL_PIXELFORMAT_RGBA128_FLOAT);
                SDL_DestroySurface(readback);
                SDL_SetRenderTarget(renderer, ptr::null_mut());
                let rgb = std::slice::from_raw_parts((*pixels).pixels.cast::<f32>(), 3).to_vec();
                SDL_DestroySurface(pixels);
                let nits = rgb.iter().map(|value| value * unit).collect::<Vec<f32>>();
                eprintln!("kind {kind}, headroom {headroom}: {nits:?} nits");
                for value in nits {
                    assert!((value - 203.0).abs() < 203.0 * 0.03, "kind {kind}: {value} nits, not HLG's reference white");
                }
            }
            SDL_DestroyTexture(target);
            SDL_DestroyRenderer(renderer);
            SDL_DestroyWindow(window);
            SDL_Quit();
        }
    }
}
