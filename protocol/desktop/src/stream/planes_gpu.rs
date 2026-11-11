//! Draws the GPU decoder's planes through SDL's GPU renderer (brief G3, Linux).
//!
//! The planes the SDK decoded sit in images on the renderer's own device (`zero_copy.rs`) or in
//! CPU buffers (`gpu_planes::Lease`). This pass draws a full-target rectangle with
//! `pyrowave-planes.spv`, the `planes` entry of pyrowave.hlsl, which converts the planes to the
//! same linear RGB as the CPU path. Images are sampled in place: each plane set is wrapped as
//! three SDL sampler textures with a render state of its own, the presenter holds the shown
//! picture's set (`gpu_planes::Handover`) in SHADER_READ_ONLY_OPTIMAL and gives it back, in
//! GENERAL with the reuse value signalled behind the last draw that read it, when the next picture
//! replaces it. With `NIGHTJAR_PYROWAVE_SAMPLE_PLANES=0` (`PlaneHandoff::Copy`) the images are
//! copied on the GPU into three persistent R8 or R16 UNORM textures of the pass instead and the
//! set goes back behind the copy, as before; CPU buffers are always uploaded into those textures.
//! Nothing here waits for the GPU: the layout changes, copies and draws are ordered by the
//! renderer's own queue.

use std::ptr;
use std::sync::Arc;
use std::time::Instant;

use sdl3_sys::everything::*;

use super::gpu_planes::{Handover, Lease, Planes, SetGpu};
use super::pyrowave_sdk::zero_copy::{PlaneHandoff, SharedImages, TEXTURE_IMAGE_PROPERTY};
use super::pyrowave_sdk::GpuConfig;
use super::stage_times::Stages;

fn device_of(renderer: *mut SDL_Renderer) -> *mut SDL_GPUDevice {
    crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_GPU_DEVICE_POINTER).cast()
}

fn plane_format(ten_bit: bool) -> SDL_GPUTextureFormat {
    if ten_bit { SDL_GPU_TEXTUREFORMAT_R16_UNORM } else { SDL_GPU_TEXTUREFORMAT_R8_UNORM }
}

/// # Safety
/// `device` must be a live GPU device of the calling thread's renderer.
unsafe fn create_shader(device: *mut SDL_GPUDevice) -> *mut SDL_GPUShader {
    SDL_CreateGPUShader(device, &super::shader::Shader::Planes.info())
}

pub struct PlanesPass {
    renderer: *mut SDL_Renderer,
    device: *mut SDL_GPUDevice,
    shader: *mut SDL_GPUShader,
    sampler: *mut SDL_GPUSampler,
    textures: [*mut SDL_GPUTexture; 3],
    transfer: *mut SDL_GPUTransferBuffer,
    /// The render state drawing `textures` (null when the pass samples the decoder's images).
    state: *mut SDL_GPURenderState,
    /// The width and height of each plane texture.
    sizes: [(u32, u32); 3],
    sample_bytes: usize,
    /// The decoder's images wrapped as textures of this device, per plane set, for the planes
    /// they belong to.
    wrapped: Option<Wrapped>,
    /// The set the shown picture is sampled from, in place (`PlaneHandoff::Sample`).
    handover: Handover<Lease>,
    /// The render states drawing the wrapped images of VA-API pictures sampled in place
    /// (`vulkan_video::Planes::sample`), by their luma texture, made on first use. The textures
    /// belong to the presenter's planes, which outlive the pass.
    sampled_states: std::collections::HashMap<usize, *mut SDL_GPURenderState>,
    /// The state of the picture sampled last, which the next draw uses; null: the planes' copies.
    sampled_active: *mut SDL_GPURenderState,
}

/// The decoder's plane images as SDL textures of the pass's device.
struct Wrapped {
    planes: Arc<Planes>,
    /// Per set, its three planes.
    sets: Vec<[*mut SDL_GPUTexture; 3]>,
    /// Sampling in place: per set, the render state drawing it (empty when the pass copies).
    states: Vec<*mut SDL_GPURenderState>,
}

/// The renderer's queue as `Handover` uses it, for the sets of `images`.
struct Sets<'a> {
    renderer: *mut SDL_Renderer,
    images: &'a SharedImages,
}

impl SetGpu for Sets<'_> {
    fn flush(&mut self) -> Result<(), String> {
        // SAFETY: The pass's live renderer, on its thread.
        if unsafe { SDL_FlushRenderer(self.renderer) } { Ok(()) } else { Err(format!("the renderer cannot flush: {}", crate::sdl::error())) }
    }

    fn sampled(&mut self, set: usize) -> Result<(), String> {
        self.images.sampled(set)
    }

    fn returned(&mut self, set: usize) -> Result<u64, String> {
        self.images.returned(set)
    }
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

    /// The pass for the decoder's `planes`: one without plane textures of its own when it samples
    /// the decoder's images in place, else `new`.
    ///
    /// # Safety
    /// As `new`.
    pub unsafe fn for_planes(renderer: *mut SDL_Renderer, planes: &Planes) -> Result<Self, String> {
        let config = planes.config();
        if !samples_in_place(planes) {
            return Self::new(renderer, config);
        }
        Self::shader_only(renderer, [0, 1, 2].map(|plane| config.plane_size(plane)), if config.ten_bit { 2 } else { 1 })
    }

    /// Whether the pass reads `planes` the way it was made for (`for_planes`): in place, or from
    /// its own textures.
    pub fn suits(&self, planes: &Planes) -> bool {
        samples_in_place(planes) == self.textures[0].is_null()
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

    /// The next draws read the planes `vulkan_video::Planes::copy` copied into (what `for_vulkan` bound).
    pub fn use_copy(&mut self) {
        self.sampled_active = ptr::null_mut();
    }

    /// The next draws read `textures`, the wrapped images of the VA-API picture sampled last
    /// (`vulkan_video::Planes::sample`), and draws them over all of `target` as `draw_bound` does
    /// (null: `draw_direct` or a later `draw_bound` draws them).
    ///
    /// # Safety
    /// As `draw_bound`, and `textures` live textures of the renderer's device that outlive the pass.
    pub unsafe fn draw_sampled_textures(&mut self, renderer: *mut SDL_Renderer, target: *mut SDL_Texture, textures: [*mut SDL_GPUTexture; 2], parameters: &[u32; 8]) -> Result<(), String> {
        let key = textures[0] as usize;
        let state = match self.sampled_states.get(&key) {
            Some(&state) => state,
            None => {
                let state = self.make_state(renderer, [textures[0], textures[1], textures[1]])?;
                self.sampled_states.insert(key, state);
                state
            }
        };
        self.sampled_active = state;
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
            renderer, device, shader: ptr::null_mut(), sampler: ptr::null_mut(), textures: [ptr::null_mut(); 3],
            transfer: ptr::null_mut(), state: ptr::null_mut(), sizes, sample_bytes, wrapped: None, handover: Handover::default(),
            sampled_states: Default::default(), sampled_active: ptr::null_mut(),
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
        self.state = self.make_state(renderer, textures)?;
        Ok(())
    }

    unsafe fn make_state(&self, renderer: *mut SDL_Renderer, textures: [*mut SDL_GPUTexture; 3]) -> Result<*mut SDL_GPURenderState, String> {
        let bindings = textures.map(|texture| SDL_GPUTextureSamplerBinding { texture, sampler: self.sampler });
        let state = SDL_CreateGPURenderState(renderer, &SDL_GPURenderStateCreateInfo {
            fragment_shader: self.shader, num_sampler_bindings: 3, sampler_bindings: bindings.as_ptr(), ..Default::default() });
        if state.is_null() {
            return Err(format!("the plane render state cannot be created: {}", crate::sdl::error()));
        }
        Ok(state)
    }

    /// The render state the next draw uses: the held set's when the pass samples the decoder's
    /// images in place, else the one of its own textures (null before the first picture).
    fn active_state(&self) -> *mut SDL_GPURenderState {
        if !self.sampled_active.is_null() {
            return self.sampled_active;
        }
        match (&self.wrapped, self.handover.held()) {
            (Some(wrapped), Some(set)) if set < wrapped.states.len() => wrapped.states[set],
            _ => self.state,
        }
    }

    /// Copies or samples the planes of `lease` (on the GPU, or as an upload) and draws them,
    /// converted, over all of `target` (the renderer's RGBA64F picture texture; null: at present
    /// time, `draw_direct`). `parameters` are the shader's constants. The CPU time of filling and
    /// submitting the copy (or the layout change) goes to `stages`' upload, that of the conversion
    /// to its passes (the GPU runs on after the submit returns).
    ///
    /// # Safety
    /// `renderer` and `target` must be live and the pass the one made on this renderer.
    pub unsafe fn draw(&mut self, renderer: *mut SDL_Renderer, target: *mut SDL_Texture, lease: &Lease, parameters: &[u32; 8], stages: &mut Stages<2>) -> Result<(), String> {
        if let Some(images) = lease.planes().images() {
            let images = images.clone();
            if !self.suits(lease.planes()) {
                return Err("the plane pass was made for the other way of reading the decoder's images".into());
            }
            if images.handoff() == PlaneHandoff::Sample {
                return self.draw_sampled(renderer, target, lease, &images, parameters, stages);
            }
            return self.draw_shared(renderer, target, lease, &images, parameters, stages);
        }
        if self.textures[0].is_null() {
            return Err("the plane pass was made for the decoder's images, not CPU planes".into());
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
        let sources = self.wrap(lease.planes(), images)?.sets[lease.set()];
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

    /// `draw` for a set sampled in place: takes the picture's lease over, makes the set readable
    /// and gives the set it replaces back behind the draws recorded so far (`Handover::show`),
    /// then draws from it (now, into `target`, or at present time).
    unsafe fn draw_sampled(&mut self, renderer: *mut SDL_Renderer, target: *mut SDL_Texture, lease: &Lease, images: &Arc<SharedImages>, parameters: &[u32; 8], stages: &mut Stages<2>) -> Result<(), String> {
        let step = Instant::now();
        self.wrap(lease.planes(), images)?;
        let next = lease.hand_over().ok_or("the picture's plane set was given back already")?;
        self.handover.show(&mut Sets { renderer, images: images.as_ref() }, next)?;
        stages.add(super::pyrowave_gpu::UPLOAD, step);
        self.convert(renderer, target, parameters, Some(stages))
    }

    /// The textures wrapping each plane set of `planes`' images (and, sampled in place, a render
    /// state per set), made on first use. Those of earlier planes (an earlier decoder's) are
    /// released first, the set held from them given back.
    unsafe fn wrap(&mut self, planes: &Arc<Planes>, images: &SharedImages) -> Result<&Wrapped, String> {
        if self.wrapped.as_ref().is_some_and(|wrapped| !Arc::ptr_eq(&wrapped.planes, planes)) {
            self.unwrap();
        }
        if self.wrapped.is_none() {
            let config = images.config();
            let sampled = images.handoff() == PlaneHandoff::Sample;
            // Sampled, SDL expects the image in SHADER_READ_ONLY_OPTIMAL whenever it draws it (the
            // held set is, `SharedImages::sampled`); as compute storage, in GENERAL, where the
            // decoder leaves it, for the copy.
            let usage = if sampled { SDL_GPU_TEXTUREUSAGE_SAMPLER } else { SDL_GPU_TEXTUREUSAGE_COMPUTE_STORAGE_WRITE };
            // Kept as it is made, so `unwrap` releases what was made on a failure.
            self.wrapped = Some(Wrapped { planes: planes.clone(), sets: Vec::new(), states: Vec::new() });
            for set in 0..config.slots {
                let mut textures = [ptr::null_mut(); 3];
                for (plane, texture) in textures.iter_mut().enumerate() {
                    let (width, height) = config.plane_size(plane);
                    let props = SDL_CreateProperties();
                    SDL_SetNumberProperty(props, TEXTURE_IMAGE_PROPERTY.as_ptr(), ash::vk::Handle::as_raw(images.image(set, plane)) as i64);
                    *texture = SDL_CreateGPUTexture(self.device, &SDL_GPUTextureCreateInfo {
                        r#type: SDL_GPU_TEXTURETYPE_2D, format: plane_format(config.ten_bit), usage,
                        width, height, layer_count_or_depth: 1, num_levels: 1, sample_count: SDL_GPU_SAMPLECOUNT_1, props });
                    SDL_DestroyProperties(props);
                }
                let failed = textures.iter().any(|texture| texture.is_null());
                if let Some(wrapped) = self.wrapped.as_mut() { wrapped.sets.push(textures); }
                let state = if failed || !sampled {
                    Ok(ptr::null_mut())
                } else {
                    self.make_state(self.renderer, textures)
                };
                match state {
                    Ok(state) if !failed => {
                        if sampled { if let Some(wrapped) = self.wrapped.as_mut() { wrapped.states.push(state); } }
                    }
                    outcome => {
                        let error = outcome.err().unwrap_or_else(crate::sdl::error);
                        self.unwrap();
                        return Err(format!("the decoder's plane images cannot be wrapped as textures: {error}"));
                    }
                }
            }
        }
        self.wrapped.as_ref().ok_or_else(|| "the decoder's plane images are not wrapped".into())
    }

    /// Gives the set the shown picture is sampled from back to the decoder, behind the draws
    /// recorded so far (flushed first).
    unsafe fn give_back_held(&mut self) {
        let Some(images) = self.wrapped.as_ref().and_then(|wrapped| wrapped.planes.images().cloned()) else { return };
        self.handover.finish(&mut Sets { renderer: self.renderer, images: images.as_ref() });
    }

    /// Gives the held set back, releases the wrapped textures and their render states and lets
    /// the images go. SDL destroys a texture's views only once its last command is done, and the
    /// images must outlive them, so this waits for that.
    unsafe fn unwrap(&mut self) {
        self.give_back_held();
        let Some(Wrapped { planes, sets, states }) = self.wrapped.take() else { return };
        for texture in sets.into_iter().flatten().filter(|texture| !texture.is_null()) {
            SDL_ReleaseGPUTexture(self.device, texture);
        }
        let idle = SDL_WaitForGPUIdle(self.device);
        for state in states.into_iter().filter(|state| !state.is_null()) {
            SDL_DestroyGPURenderState(state);
        }
        if !idle {
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
        let drawn = SDL_SetGPURenderState(renderer, self.active_state()) && SDL_RenderFillRect(renderer, ptr::null()) && SDL_FlushRenderer(renderer);
        let error = if drawn { String::new() } else { crate::sdl::error() };
        SDL_SetGPURenderState(renderer, ptr::null_mut());
        SDL_SetRenderTarget(renderer, previous);
        if let Some(stages) = stages { stages.add(super::pyrowave_gpu::PASSES, step); }
        if drawn { Ok(()) } else { Err(format!("the planes cannot be drawn: {error}")) }
    }
}

impl PlanesPass {
    unsafe fn set_uniforms(&mut self, uniforms: &Uniforms) -> Result<(), String> {
        let state = self.active_state();
        if state.is_null() {
            return Err("no picture's planes to draw".into());
        }
        if !SDL_SetGPURenderStateFragmentUniforms(state, 0, (uniforms as *const Uniforms).cast(), std::mem::size_of::<Uniforms>() as u32) {
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
        let drawn = SDL_SetGPURenderState(renderer, self.active_state()) && SDL_RenderFillRect(renderer, destination);
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
    /// What flag 8192 reads: the gamma 2.2 range's minimum and 1 / (peak - minimum), in nits.
    pub encoding: [f32; 4],
}

/// The shader's flag for a draw straight into the target rectangle.
pub const DIRECT_FLAG: u32 = 512;
/// With `DIRECT_FLAG`: the target is sRGB, not scRGB, so the shader encodes and clamps as SDL does.
pub const SRGB_TARGET_FLAG: u32 = 1024;
/// With `DIRECT_FLAG`: the target is the HDR10 swapchain image of a video-only frame (the patched
/// SDL's `SDL.renderer.gpu.video_only`), so the shader finishes with the PQ present pass's encode
/// (`pq_present.hlsl`: BT.709 to BT.2020, 1.0 = 80 nits, ST 2084) of the scRGB it would write.
pub const PQ_TARGET_FLAG: u32 = 4096;
/// With `PQ_TARGET_FLAG`: the swapchain is drawn in the compositor's gamma 2.2 blending space, as
/// the scanout present pass would (`scanout_present.hlsl`, `wl_scanout::Encoding::Gamma22`).
pub const GAMMA22_TARGET_FLAG: u32 = 8192;

/// What SDL's draw of the RGBA64F linear picture texture into the back buffer does to it
/// (`texture_advanced.frag.hlsl`): its tone map, when the texture's HDR headroom is above the
/// output's, then its colour scale (the output's SDR white times the renderer's colour scale),
/// then on an sRGB output (an SDR display outside gamescope) its sRGB encoding and clamp.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SdlLook {
    pub texture_headroom: f32,
    pub output_headroom: f32,
    pub sdr_white: f32,
    pub color_scale: f32,
    /// The output is scRGB (`SDL_RenderingLinearSpace`), not sRGB.
    pub linear_output: bool,
    /// The frame is video only on a PQ output: the draw writes PQ, not scRGB (`PQ_TARGET_FLAG`).
    pub pq_target: bool,
    /// The encoding the renderer's present writes into its swapchain (`sdl::present_encoding`):
    /// mode 1 makes a `pq_target` draw gamma 2.2 instead (`GAMMA22_TARGET_FLAG`).
    pub target_encoding: [f32; 4],
}

impl SdlLook {
    /// What SDL's draw of `texture` into the renderer's current target (the back buffer, or a
    /// target texture) applies, read as `CalculateAdvancedShaderConstants` and
    /// `SDL_RenderingLinearSpace` read it.
    ///
    /// # Safety
    /// `renderer` and `texture` must be live, on the calling thread.
    pub unsafe fn of(renderer: *mut SDL_Renderer, texture: *mut SDL_Texture) -> Self {
        let target = SDL_GetRenderTarget(renderer);
        let (output, colorspace, sdr_white, output_headroom) = if target.is_null() {
            let props = SDL_GetRendererProperties(renderer);
            (props, SDL_GetNumberProperty(props, SDL_PROP_RENDERER_OUTPUT_COLORSPACE_NUMBER, 0),
                SDL_PROP_RENDERER_SDR_WHITE_POINT_FLOAT, SDL_PROP_RENDERER_HDR_HEADROOM_FLOAT)
        } else {
            let props = SDL_GetTextureProperties(target);
            (props, SDL_GetNumberProperty(props, SDL_PROP_TEXTURE_COLORSPACE_NUMBER, 0),
                SDL_PROP_TEXTURE_SDR_WHITE_POINT_FLOAT, SDL_PROP_TEXTURE_HDR_HEADROOM_FLOAT)
        };
        let mut color_scale = 1.0;
        SDL_GetRenderColorScale(renderer, &mut color_scale);
        Self {
            // SDL leaves a texture's headroom out when it is 0, its default for the linear format.
            texture_headroom: SDL_GetFloatProperty(SDL_GetTextureProperties(texture), SDL_PROP_TEXTURE_HDR_HEADROOM_FLOAT, 0.0),
            output_headroom: SDL_GetFloatProperty(output, output_headroom, 0.0),
            sdr_white: SDL_GetFloatProperty(output, sdr_white, 1.0),
            color_scale,
            linear_output: colorspace == i64::from(SDL_COLORSPACE_SRGB_LINEAR.0),
            pq_target: false,
            target_encoding: crate::sdl::present_encoding(renderer),
        }
    }

    /// SDL's TONEMAP_CHROME factors (`CalculateAdvancedShaderConstants`), None without a tone map.
    pub fn tonemap(&self) -> Option<(f32, f32)> {
        (self.texture_headroom > self.output_headroom && self.output_headroom > 0.0)
            .then(|| (self.output_headroom / (self.texture_headroom * self.texture_headroom), 1.0 / self.output_headroom))
    }
}

/// The constants of a `draw_direct` into `destination` (render coordinates) of a viewport at `origin`.
pub fn direct_uniforms(parameters: &[u32; 8], destination: &SDL_FRect, origin: (i32, i32), visible: (i32, i32), look: SdlLook) -> Uniforms {
    let mut parameters = *parameters;
    // A PQ target takes the scRGB a linear one would, encoded: the output colorspace stays scRGB.
    parameters[6] |= DIRECT_FLAG | if look.pq_target { PQ_TARGET_FLAG } else if look.linear_output { 0 } else { SRGB_TARGET_FLAG };
    // The swapchain the present would draw in gamma 2.2 gets the same encode from this draw.
    let [mode, min_nits, inv_range, _] = look.target_encoding;
    let gamma22 = look.pq_target && mode == 1.0 && inv_range > 0.0;
    if gamma22 {
        parameters[6] |= GAMMA22_TARGET_FLAG;
    }
    let (tonemap1, tonemap2) = look.tonemap().unwrap_or((0.0, 0.0));
    Uniforms {
        parameters,
        destination: [destination.x + origin.0 as f32, destination.y + origin.1 as f32, destination.w.max(1.0), destination.h.max(1.0)],
        picture: [visible.0 as f32, visible.1 as f32, 0.0, 0.0],
        output_scale: look.sdr_white * look.color_scale,
        tonemap: [tonemap1, tonemap2],
        unused: 0.0,
        encoding: if gamma22 { [min_nits, inv_range, 0.0, 0.0] } else { [0.0; 4] },
    }
}

/// Whether the presenter samples `planes` in place (images on the renderer's device, not copied).
fn samples_in_place(planes: &Planes) -> bool {
    planes.images().is_some_and(|images| images.handoff() == PlaneHandoff::Sample)
}

/// Whether a picture's chroma is one interleaved plane (NV12, P010).
fn semi_planar(info: &crate::media::FrameInfo) -> bool {
    matches!(info.kind, crate::media::FRAME_NV12 | crate::media::FRAME_P010)
}

/// Whether a planar picture's 16-bit samples hold its 10-bit codes in their high bits, as P010's
/// do: NVDEC's 10-bit 4:4:4 copied to memory (`media::FRAME_YUV444P16`).
fn high_bit_planes(info: &crate::media::FrameInfo) -> bool {
    info.kind == crate::media::FRAME_YUV444P16
}

/// Copies a picture's planes into the transfer buffer as the tightly packed planes `sizes` gives.
/// A planar picture's rows go as they are. An NV12 or P010 picture's interleaved chroma is split
/// into its Cb and Cr planes, and P010's samples, in the high 10 bits of each word, move to the low
/// bits, where the shader reads FFmpeg's 10-bit planes (flag 64); so do the samples of a planar
/// picture with high-bit planes (`high_bit_planes`).
///
/// # Safety
/// `info` describes a live frame with at least `sizes` samples in each plane, and `mapped` has
/// room for the planes at `offsets`.
unsafe fn fill_planes(info: &crate::media::FrameInfo, sizes: [(u32, u32); 3], sample_bytes: usize, mapped: *mut u8, offsets: [usize; 3]) {
    if !semi_planar(info) {
        let high_bits = sample_bytes == 2 && high_bit_planes(info);
        for (plane, &(width, height)) in sizes.iter().enumerate() {
            let row = width as usize * sample_bytes;
            for y in 0..height as usize {
                let source = info.data[plane].add(y * info.linesize[plane] as usize);
                let target = mapped.add(offsets[plane] + y * row);
                if high_bits {
                    let source = std::slice::from_raw_parts(source, row);
                    let target = std::slice::from_raw_parts_mut(target, row);
                    for (target, source) in target.chunks_exact_mut(2).zip(source.chunks_exact(2)) {
                        target.copy_from_slice(&(u16::from_le_bytes([source[0], source[1]]) >> 6).to_le_bytes());
                    }
                } else {
                    ptr::copy_nonoverlapping(source, target, row);
                }
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
        // SAFETY: These objects belong to the renderer's device, which outlives the pass; the
        // renderer outlives it too, so the held set is given back behind its last draw.
        unsafe {
            self.unwrap();
            if !self.sampled_states.is_empty() {
                // The held picture's draws may still read the images the states are of.
                SDL_WaitForGPUIdle(self.device);
                for (_, state) in self.sampled_states.drain() { SDL_DestroyGPURenderState(state); }
            }
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

    const SDR: SdlLook = SdlLook { texture_headroom: 0.0, output_headroom: 1.0, sdr_white: 1.0, color_scale: 1.0, linear_output: true, pq_target: false, target_encoding: [0.0; 4] };

    /// planesDirect after the conversion, in the shader's order, for checking its numbers.
    fn shader_output(rgb: [f32; 3], uniforms: &Uniforms) -> [f32; 3] {
        let mul = |m: [[f32; 3]; 3], v: [f32; 3]| m.map(|row| row[0] * v[0] + row[1] * v[1] + row[2] * v[2]);
        let to2020 = [[0.627404, 0.329283, 0.043313], [0.069097, 0.919541, 0.011362], [0.016391, 0.088013, 0.895595]];
        let to709 = [[1.660496, -0.587656, -0.072840], [-0.124547, 1.132895, -0.008348], [-0.018154, -0.100597, 1.118751]];
        let [tonemap1, tonemap2] = uniforms.tonemap;
        let mut rgb = rgb;
        if tonemap2 > 0.0 {
            let mut v = mul(to2020, rgb);
            let vmax = v[0].max(v[1]).max(v[2]);
            if vmax > 0.0 { v = v.map(|c| c * (1.0 + tonemap1 * vmax) / (1.0 + tonemap2 * vmax)); }
            rgb = mul(to709, v);
        }
        rgb = rgb.map(|c| c * uniforms.output_scale);
        if uniforms.parameters[6] & GAMMA22_TARGET_FLAG != 0 {
            return gamma22_of_scrgb(rgb, uniforms.encoding);
        }
        if uniforms.parameters[6] & PQ_TARGET_FLAG != 0 {
            return pq_of_scrgb(rgb);
        }
        if uniforms.parameters[6] & SRGB_TARGET_FLAG != 0 {
            rgb = rgb.map(|c| if c <= 0.0031308 { c * 12.92 } else { c.abs().powf(1.0 / 2.4) * 1.055 - 0.055 }.clamp(0.0, 1.0));
        }
        rgb
    }

    /// The bug: on an sRGB back buffer (an SDR display) the direct draw wrote linear light, which
    /// the display reads as sRGB, so the picture came out too dark. SDL encodes and clamps.
    #[test]
    fn an_srgb_target_gets_the_picture_srgb_encoded() {
        let rect = SDL_FRect { x: 0.0, y: 0.0, w: 4.0, h: 4.0 };
        let srgb = SdlLook { linear_output: false, ..SDR };
        let encoded = direct_uniforms(&[0; 8], &rect, (0, 0), (4, 4), srgb);
        assert_eq!(encoded.parameters[6], DIRECT_FLAG | SRGB_TARGET_FLAG);
        let mid = shader_output([0.18; 3], &encoded);
        assert!((mid[0] - 0.4613).abs() < 0.001, "18 % grey is sRGB 0.46, not {}", mid[0]);
        assert_eq!(shader_output([0.001; 3], &encoded)[0], 0.001 * 12.92, "the linear toe");
        assert_eq!(shader_output([2.0, -0.5, 4.0], &encoded), [1.0, 0.0, 1.0], "clamped, as SDL saturates");
        let linear = direct_uniforms(&[0; 8], &rect, (0, 0), (4, 4), SDR);
        assert_eq!(linear.parameters[6], DIRECT_FLAG, "an scRGB target takes linear light");
        assert_eq!(shader_output([0.18; 3], &linear), [0.18; 3]);
        // HLG tone mapped onto an SDR display: tone map, then the encoding.
        let hlg = SdlLook { texture_headroom: 4.0, ..srgb };
        let mapped = shader_output([1.0; 3], &direct_uniforms(&[0; 8], &rect, (0, 0), (4, 4), hlg));
        // (1 + 1/16) / (1 + 1) of white, sRGB encoded.
        assert!(mapped.iter().all(|c| (c - 0.7556).abs() < 0.002), "white is tone mapped below the clip, then encoded: {mapped:?}");
    }

    /// BT.709 to BT.2020 primaries, as the present shaders write it, digit for digit.
    #[allow(clippy::excessive_precision)]
    const TO_2020: [[f32; 3]; 3] = [[0.6274040, 0.3292820, 0.0433136], [0.0690970, 0.9195400, 0.0113612], [0.0163916, 0.0880132, 0.8955950]];

    /// The PQ present pass's encode of scRGB (`pq_present.hlsl`), written apart from the shader's.
    fn pq_of_scrgb(rgb: [f32; 3]) -> [f32; 3] {
        let to2020 = TO_2020;
        let pq = |nits: f32| {
            // SMPTE ST 2084's constants, as it defines them.
            let (m1, m2) = (2610.0f32 / 16384.0, 2523.0f32 / 4096.0 * 128.0);
            let (c1, c2, c3) = (3424.0f32 / 4096.0, 2413.0f32 / 4096.0 * 32.0, 2392.0f32 / 4096.0 * 32.0);
            let y = (nits / 10000.0).clamp(0.0, 1.0).powf(m1);
            ((c1 + c2 * y) / (1.0 + c3 * y)).powf(m2)
        };
        to2020.map(|row| pq((row[0] * rgb[0] + row[1] * rgb[1] + row[2] * rgb[2]).max(0.0) * 80.0))
    }

    /// The scanout present's gamma 2.2 encode of scRGB (`scanout_present.hlsl`) before its dither,
    /// written apart from the shader's: BT.2020 nits over the range `[min, 1 / (peak - min)]`.
    fn gamma22_of_scrgb(rgb: [f32; 3], encoding: [f32; 4]) -> [f32; 3] {
        let to2020 = TO_2020;
        let [min_nits, inv_range, _, _] = encoding;
        let peak = min_nits + 1.0 / inv_range;
        to2020.map(|row| crate::stream::wl_scanout::Encoding::gamma22_code((row[0] * rgb[0] + row[1] * rgb[1] + row[2] * rgb[2]) * 80.0, min_nits, peak))
    }

    /// A video-only frame on a swapchain the present draws in the compositor's gamma 2.2 blending
    /// space (KWin 6.5 to 6.8): the draw encodes as that present would, never PQ.
    #[test]
    fn a_gamma22_target_gets_the_scrgb_picture_in_the_blending_space() {
        let rect = SDL_FRect { x: 0.0, y: 0.0, w: 4.0, h: 4.0 };
        // KWin's blending space on a 700-nit output with reference white 250 (3.125 in scRGB).
        let encoding = crate::stream::wl_scanout::Encoding::constants(1, 0.005, 700.0);
        let look = SdlLook { texture_headroom: 0.0, output_headroom: 2.8, sdr_white: 3.125, color_scale: 1.0, linear_output: true, pq_target: true, target_encoding: encoding };
        let uniforms = direct_uniforms(&[0; 8], &rect, (0, 0), (4, 4), look);
        assert_eq!(uniforms.parameters[6], DIRECT_FLAG | PQ_TARGET_FLAG | GAMMA22_TARGET_FLAG);
        assert_eq!(uniforms.encoding, [0.005, encoding[2], 0.0, 0.0], "the range, as the present gets it");
        let white = shader_output([1.0; 3], &uniforms);
        let expected = (249.995f32 / 699.995).powf(1.0 / 2.2);
        assert!(white.iter().all(|c| (c - expected).abs() < 0.0005), "reference white at the output's reference: {white:?}");
        assert_eq!(shader_output([0.0; 3], &uniforms), [0.0; 3], "black is the range's minimum");
        assert_eq!(shader_output([10.0; 3], &uniforms), [1.0; 3], "above the peak clips");
        // The PQ encoding, an empty range and a draw that is not video only stay as they were.
        let pq = direct_uniforms(&[0; 8], &rect, (0, 0), (4, 4), SdlLook { target_encoding: [0.0; 4], ..look });
        assert_eq!((pq.parameters[6], pq.encoding), (DIRECT_FLAG | PQ_TARGET_FLAG, [0.0; 4]));
        let empty = SdlLook { target_encoding: crate::stream::wl_scanout::Encoding::constants(1, 700.0, 700.0), ..look };
        assert_eq!(direct_uniforms(&[0; 8], &rect, (0, 0), (4, 4), empty).parameters[6], DIRECT_FLAG | PQ_TARGET_FLAG);
        let scrgb = direct_uniforms(&[0; 8], &rect, (0, 0), (4, 4), SdlLook { pq_target: false, ..look });
        assert_eq!((scrgb.parameters[6], scrgb.encoding), (DIRECT_FLAG, [0.0; 4]), "the back buffer is scRGB; the present encodes it");
    }

    /// A video-only frame on a PQ output: the draw keeps every step of the scRGB one (scale, tone
    /// map) and finishes with the PQ pass's encode, and never takes the sRGB encoding.
    #[test]
    fn a_pq_target_gets_the_scrgb_picture_pq_encoded() {
        let rect = SDL_FRect { x: 0.0, y: 0.0, w: 4.0, h: 4.0 };
        // An HDR output: 203-nit SDR white (2.5375 in scRGB), 1000-nit peak.
        let hdr = SdlLook { texture_headroom: 0.0, output_headroom: 12.5, sdr_white: 2.5375, color_scale: 1.0, linear_output: true, pq_target: true, target_encoding: [0.0; 4] };
        let uniforms = direct_uniforms(&[0; 8], &rect, (0, 0), (4, 4), hdr);
        assert_eq!(uniforms.parameters[6], DIRECT_FLAG | PQ_TARGET_FLAG);
        assert_eq!(uniforms.output_scale, 2.5375, "the scale is the scRGB draw's");
        let white = shader_output([1.0; 3], &uniforms);
        // 203 nits is ST 2084 code 0.5807 (BT.2408's reference white).
        assert!(white.iter().all(|c| (c - 0.5807).abs() < 0.0005), "SDR white: {white:?}");
        assert_eq!(shader_output([0.0; 3], &uniforms), [pq_of_scrgb([0.0; 3])[0]; 3], "black is PQ's black, not 0 after a clamp of its own");
        let red = shader_output([1.0, 0.0, 0.0], &uniforms);
        assert!(red[0] > red[1] && red[1] > 0.0 && red[2] > 0.0, "BT.709 red lies inside BT.2020: {red:?}");
        // Without a PQ target, an sRGB output still takes the sRGB flag.
        let srgb = direct_uniforms(&[0; 8], &rect, (0, 0), (4, 4), SdlLook { linear_output: false, pq_target: false, ..hdr });
        assert_eq!(srgb.parameters[6], DIRECT_FLAG | SRGB_TARGET_FLAG);
        assert_eq!(direct_uniforms(&[0; 8], &rect, (0, 0), (4, 4), SdlLook { pq_target: false, ..hdr }).parameters[6], DIRECT_FLAG);
    }

    /// The constants are the shader's 96-byte `Picture` block, in its order.
    #[test]
    fn the_direct_constants_match_the_shader_block() {
        assert_eq!(std::mem::size_of::<Uniforms>(), 96);
        assert_eq!(std::mem::offset_of!(Uniforms, destination), 32);
        assert_eq!(std::mem::offset_of!(Uniforms, picture), 48);
        assert_eq!(std::mem::offset_of!(Uniforms, output_scale), 64);
        assert_eq!(std::mem::offset_of!(Uniforms, tonemap), 68);
        assert_eq!(std::mem::offset_of!(Uniforms, encoding), 80);
        let shader = include_str!("../pyrowave.hlsl");
        let rows = shader.lines().map(str::trim).collect::<Vec<_>>();
        let at = |row: &str| rows.iter().position(|line| *line == row).unwrap_or_else(|| panic!("no {row}"));
        assert_eq!(at("float minNits; float invRange; float2 unusedEncoding;"), at("float outputScale; float tonemap1; float tonemap2; float unused;") + 2,
            "the encoding's row follows the row at 64 (after its comment)");
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
        let hdr = SdlLook { texture_headroom: 12.0, output_headroom: 4.0, sdr_white: 2.5, color_scale: 0.5, linear_output: true, pq_target: false, target_encoding: [0.0; 4] };
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

    /// NVDEC's 10-bit 4:4:4 in memory (YUV444P16) holds its codes in the high bits, as P010 does:
    /// the upload moves them to the low bits the shader reads (flag 64), so a code comes out as
    /// itself, not 64 times too bright. FFmpeg's own 10-bit planes go as they are.
    #[test]
    fn high_bit_planes_are_uploaded_with_their_codes_in_the_low_bits() {
        use crate::media::{FRAME_YUV444P10, FRAME_YUV444P16};
        let words = |w: [u16; 4]| w.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>();
        // 3x1 planes in rows of 4 words, the last padding.
        let mut planes = [words([64 << 6, 940 << 6, 1023 << 6, 0xffff]), words([512 << 6, 1 << 6, 0, 0xffff]), words([960 << 6, 513 << 6, 2 << 6, 0xffff])];
        let mut info = FrameInfo {
            width: 3, height: 1, kind: FRAME_YUV444P16, ten_bit: 1, pts: 0,
            data: [planes[0].as_mut_ptr(), planes[1].as_mut_ptr(), planes[2].as_mut_ptr()],
            linesize: [8; 3], color_space: 1, color_range: 1, color_trc: 1,
        };
        let layout = PlanarLayout::of(&info).unwrap();
        assert_eq!(layout, PlanarLayout { width: 3, height: 1, chroma444: true, ten_bit: true });
        assert!(high_bit_planes(&info));
        let sizes = [0, 1, 2].map(|plane| layout.plane_size(plane));
        let fill = |info: &FrameInfo| {
            let mut buffer = vec![0xaau8; 9 * 2];
            // SAFETY: the planes above hold `sizes` samples; the buffer holds the three planes.
            unsafe { fill_planes(info, sizes, 2, buffer.as_mut_ptr(), [0, 6, 12]) };
            buffer.chunks_exact(2).map(|s| u16::from_le_bytes([s[0], s[1]])).collect::<Vec<u16>>()
        };
        assert_eq!(fill(&info), [64, 940, 1023, 512, 1, 0, 960, 513, 2]);
        // FFmpeg's software 10-bit 4:4:4 is already in the low bits: copied as it is.
        info.kind = FRAME_YUV444P10;
        assert!(!high_bit_planes(&info));
        assert_eq!(fill(&info), [64 << 6, 940 << 6, 1023 << 6, 512 << 6, 1 << 6, 0, 960 << 6, 513 << 6, 2 << 6]);
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
    /// The direct draw gives what the texture path gives (the plane pass into the RGBA64F picture
    /// texture, then SDL's draw of it), on an sRGB target, the SDR back buffer's format, and on an
    /// scRGB one, with and without SDL's tone map: `cargo test -p nightjar-desktop -- --ignored
    /// stream::planes_gpu`.
    #[test]
    #[ignore = "needs a Vulkan device and SDL's offscreen video driver"]
    fn the_direct_draw_matches_the_texture_path() {
        let (width, height) = (16usize, 8usize);
        // A ramp in luma and both chroma planes, so matrix, range and sampling all show.
        let mut luma = (0..width * height).map(|i| (16 + (i * 219) / (width * height)) as u8).collect::<Vec<u8>>();
        let mut chroma = (0..width * height / 2).map(|i| if i % 2 == 0 { (64 + i * 2) as u8 } else { (192 - i * 2) as u8 }).collect::<Vec<u8>>();
        // SAFETY: Plain SDL calls on objects made here, on one thread.
        unsafe {
            SDL_SetHint(SDL_HINT_VIDEO_DRIVER, c"offscreen".as_ptr());
            assert!(SDL_Init(SDL_INIT_VIDEO), "{}", crate::sdl::error());
            let window = SDL_CreateWindow(c"direct planes".as_ptr(), 64, 64, SDL_WINDOW_HIDDEN);
            let renderer = crate::sdl::create_renderer(window, true);
            assert!(!renderer.is_null(), "{}", crate::sdl::error());
            assert_eq!(crate::sdl::renderer_name(renderer), "gpu");
            let mut info = picture(FRAME_NV12, &mut luma, &mut chroma, width as i32);
            (info.width, info.height) = (width as i32, height as i32);
            let layout = PlanarLayout::of_semi_planar(&info).unwrap();
            let parameters = super::super::present::planar_parameters(&info, layout, false, 1.0, 1.0);
            let texture = |format: SDL_PixelFormat, space: SDL_Colorspace, headroom: Option<f32>| {
                let props = SDL_CreateProperties();
                SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_FORMAT_NUMBER, i64::from(format.0));
                SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_ACCESS_NUMBER, i64::from(SDL_TEXTUREACCESS_TARGET.0));
                SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_WIDTH_NUMBER, width as i64);
                SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_HEIGHT_NUMBER, height as i64);
                SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_COLORSPACE_NUMBER, i64::from(space.0));
                if let Some(headroom) = headroom { SDL_SetFloatProperty(props, SDL_PROP_TEXTURE_CREATE_HDR_HEADROOM_FLOAT, headroom); }
                let texture = SDL_CreateTextureWithProperties(renderer, props);
                SDL_DestroyProperties(props);
                assert!(!texture.is_null(), "{}", crate::sdl::error());
                texture
            };
            let read = |target: *mut SDL_Texture| {
                assert!(SDL_SetRenderTarget(renderer, target), "{}", crate::sdl::error());
                let readback = SDL_RenderReadPixels(renderer, ptr::null());
                assert!(!readback.is_null(), "{}", crate::sdl::error());
                let pixels = SDL_ConvertSurface(readback, SDL_PIXELFORMAT_RGBA128_FLOAT);
                SDL_DestroySurface(readback);
                let values = std::slice::from_raw_parts((*pixels).pixels.cast::<f32>(), width * height * 4).to_vec();
                SDL_DestroySurface(pixels);
                values
            };
            let dst = SDL_FRect { x: 0.0, y: 0.0, w: width as f32, h: height as f32 };
            let targets = [("sRGB", SDL_PIXELFORMAT_RGBA32, SDL_COLORSPACE_SRGB), ("scRGB", SDL_PIXELFORMAT_RGBA64_FLOAT, SDL_COLORSPACE_SRGB_LINEAR)];
            for ((name, format, space), headroom) in targets.into_iter().flat_map(|target| [None, Some(4.0)].map(|headroom| (target, headroom))) {
                let pictured = texture(SDL_PIXELFORMAT_RGBA64_FLOAT, SDL_COLORSPACE_SRGB_LINEAR, headroom);
                let (through_texture, direct) = (texture(format, space, None), texture(format, space, None));
                let mut pass = PlanesPass::for_frames(renderer, layout).unwrap();
                pass.draw_frame(renderer, pictured, &info, &parameters).unwrap();
                assert!(SDL_SetRenderTarget(renderer, through_texture), "{}", crate::sdl::error());
                assert!(SDL_RenderTexture(renderer, pictured, ptr::null(), &dst), "{}", crate::sdl::error());
                let expected = read(through_texture);
                pass.draw_frame(renderer, ptr::null_mut(), &info, &parameters).unwrap();
                assert!(SDL_SetRenderTarget(renderer, direct), "{}", crate::sdl::error());
                let look = SdlLook::of(renderer, pictured);
                assert_eq!(look.linear_output, space == SDL_COLORSPACE_SRGB_LINEAR);
                pass.draw_direct(renderer, &parameters, &dst, (width as i32, height as i32), look).unwrap();
                let drawn = read(direct);
                SDL_SetRenderTarget(renderer, ptr::null_mut());
                let worst = expected.iter().zip(&drawn).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
                eprintln!("{name}, texture headroom {headroom:?} ({look:?}): largest difference {worst}");
                assert!(worst < 0.01, "{name}, headroom {headroom:?}: the direct draw differs from SDL's by {worst}");
                drop(pass);
                for texture in [pictured, through_texture, direct] { SDL_DestroyTexture(texture); }
            }
            SDL_DestroyRenderer(renderer);
            SDL_DestroyWindow(window);
            SDL_Quit();
        }
    }

    /// A video-only frame's draw (`PQ_TARGET_FLAG`) into an RGB10A2 target, the HDR10 swapchain
    /// image's format, is the PQ pass's encode of the scRGB direct draw, within a 10-bit code and a
    /// half: `cargo test -p nightjar-desktop -- --ignored stream::planes_gpu`.
    #[test]
    #[ignore = "needs a Vulkan device and SDL's offscreen video driver"]
    fn the_pq_draw_is_the_pq_pass_of_the_scrgb_draw() {
        let (width, height) = (16usize, 8usize);
        let mut luma = (0..width * height).map(|i| (16 + (i * 219) / (width * height)) as u8).collect::<Vec<u8>>();
        let mut chroma = (0..width * height / 2).map(|i| if i % 2 == 0 { (64 + i * 2) as u8 } else { (192 - i * 2) as u8 }).collect::<Vec<u8>>();
        // SAFETY: Plain SDL calls on objects made here, on one thread.
        unsafe {
            SDL_SetHint(SDL_HINT_VIDEO_DRIVER, c"offscreen".as_ptr());
            assert!(SDL_Init(SDL_INIT_VIDEO), "{}", crate::sdl::error());
            let window = SDL_CreateWindow(c"PQ planes".as_ptr(), 64, 64, SDL_WINDOW_HIDDEN);
            let renderer = crate::sdl::create_renderer(window, true);
            assert!(!renderer.is_null(), "{}", crate::sdl::error());
            assert_eq!(crate::sdl::renderer_name(renderer), "gpu");
            let mut info = picture(FRAME_NV12, &mut luma, &mut chroma, width as i32);
            (info.width, info.height) = (width as i32, height as i32);
            let layout = PlanarLayout::of_semi_planar(&info).unwrap();
            let parameters = super::super::present::planar_parameters(&info, layout, false, 1.0, 1.0);
            let texture = |format: SDL_PixelFormat, space: SDL_Colorspace| {
                let props = SDL_CreateProperties();
                SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_FORMAT_NUMBER, i64::from(format.0));
                SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_ACCESS_NUMBER, i64::from(SDL_TEXTUREACCESS_TARGET.0));
                SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_WIDTH_NUMBER, width as i64);
                SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_HEIGHT_NUMBER, height as i64);
                SDL_SetNumberProperty(props, SDL_PROP_TEXTURE_CREATE_COLORSPACE_NUMBER, i64::from(space.0));
                let texture = SDL_CreateTextureWithProperties(renderer, props);
                SDL_DestroyProperties(props);
                assert!(!texture.is_null(), "{}", crate::sdl::error());
                texture
            };
            let scrgb = texture(SDL_PIXELFORMAT_RGBA64_FLOAT, SDL_COLORSPACE_SRGB_LINEAR);
            let pq = texture(SDL_PIXELFORMAT_ABGR2101010, SDL_COLORSPACE_HDR10);
            let dst = SDL_FRect { x: 0.0, y: 0.0, w: width as f32, h: height as f32 };
            // An HDR output's look: 203-nit SDR white over 1.0 = 80 nits, so the ramp spans most codes.
            let look = SdlLook { texture_headroom: 0.0, output_headroom: 12.5, sdr_white: 2.5375, color_scale: 1.0, linear_output: true, pq_target: false, target_encoding: [0.0; 4] };
            let mut pass = PlanesPass::for_frames(renderer, layout).unwrap();
            pass.draw_frame(renderer, ptr::null_mut(), &info, &parameters).unwrap();
            assert!(SDL_SetRenderTarget(renderer, scrgb), "{}", crate::sdl::error());
            pass.draw_direct(renderer, &parameters, &dst, (width as i32, height as i32), look).unwrap();
            let readback = SDL_RenderReadPixels(renderer, ptr::null());
            assert!(!readback.is_null(), "{}", crate::sdl::error());
            // RGBA64F to floats, both scRGB.
            let floats = SDL_ConvertSurface(readback, SDL_PIXELFORMAT_RGBA128_FLOAT);
            SDL_DestroySurface(readback);
            assert!(!floats.is_null(), "{}", crate::sdl::error());
            let linear = std::slice::from_raw_parts((*floats).pixels.cast::<f32>(), width * height * 4).to_vec();
            SDL_DestroySurface(floats);
            assert!(SDL_SetRenderTarget(renderer, pq), "{}", crate::sdl::error());
            pass.draw_direct(renderer, &parameters, &dst, (width as i32, height as i32), SdlLook { pq_target: true, ..look }).unwrap();
            let readback = SDL_RenderReadPixels(renderer, ptr::null());
            assert!(!readback.is_null(), "{}", crate::sdl::error());
            assert_eq!((*readback).format.0, SDL_PIXELFORMAT_ABGR2101010.0, "read back as drawn, without a conversion");
            let pitch = (*readback).pitch as usize;
            // ABGR2101010 is one 32-bit word: red in its low 10 bits, then green, then blue.
            let codes = (0..height).flat_map(|y| (0..width).map(move |x| (y, x)))
                .map(|(y, x)| {
                    let word = (*readback).pixels.cast::<u8>().add(y * pitch + x * 4).cast::<u32>().read_unaligned();
                    [word & 0x3ff, (word >> 10) & 0x3ff, (word >> 20) & 0x3ff]
                })
                .collect::<Vec<[u32; 3]>>();
            SDL_DestroySurface(readback);
            SDL_SetRenderTarget(renderer, ptr::null_mut());
            let mut worst = 0.0f32;
            for (pixel, drawn) in linear.chunks_exact(4).zip(&codes) {
                let expected = pq_of_scrgb([pixel[0], pixel[1], pixel[2]]);
                for (want, got) in expected.iter().zip(drawn) {
                    worst = worst.max((want * 1023.0 - *got as f32).abs());
                }
            }
            eprintln!("PQ draw against the PQ pass of the scRGB draw: largest difference {worst} codes");
            assert!(worst <= 1.5, "the PQ draw differs from the PQ pass of the scRGB draw by {worst} 10-bit codes");
            assert!(codes.iter().flatten().any(|&code| code > 500), "the ramp reaches up to about 203-nit white's code 594");
            // The same draw in the compositor's gamma 2.2 blending space (flag 8192), 0.005 to 700
            // nits: the scanout present's encode, within its half code of dither and a code.
            let encoding = crate::stream::wl_scanout::Encoding::constants(1, 0.005, 700.0);
            assert!(SDL_SetRenderTarget(renderer, pq), "{}", crate::sdl::error());
            pass.draw_direct(renderer, &parameters, &dst, (width as i32, height as i32), SdlLook { pq_target: true, target_encoding: encoding, ..look }).unwrap();
            let readback = SDL_RenderReadPixels(renderer, ptr::null());
            assert!(!readback.is_null(), "{}", crate::sdl::error());
            let pitch = (*readback).pitch as usize;
            let codes = (0..height).flat_map(|y| (0..width).map(move |x| (y, x)))
                .map(|(y, x)| {
                    let word = (*readback).pixels.cast::<u8>().add(y * pitch + x * 4).cast::<u32>().read_unaligned();
                    [word & 0x3ff, (word >> 10) & 0x3ff, (word >> 20) & 0x3ff]
                })
                .collect::<Vec<[u32; 3]>>();
            SDL_DestroySurface(readback);
            SDL_SetRenderTarget(renderer, ptr::null_mut());
            let mut worst = 0.0f32;
            for (pixel, drawn) in linear.chunks_exact(4).zip(&codes) {
                let expected = gamma22_of_scrgb([pixel[0], pixel[1], pixel[2]], [encoding[1], encoding[2], 0.0, 0.0]);
                for (want, got) in expected.iter().zip(drawn) {
                    worst = worst.max((want * 1023.0 - *got as f32).abs());
                }
            }
            eprintln!("gamma 2.2 draw against the scanout pass's encode of the scRGB draw: largest difference {worst} codes");
            assert!(worst <= 1.5, "the gamma 2.2 draw differs from the scanout pass's encode by {worst} 10-bit codes");
            drop(pass);
            for texture in [scrgb, pq] { SDL_DestroyTexture(texture); }
            SDL_DestroyRenderer(renderer);
            SDL_DestroyWindow(window);
            SDL_Quit();
        }
    }
}
