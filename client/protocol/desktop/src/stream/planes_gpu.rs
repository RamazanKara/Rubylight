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
    /// The plane textures `state` draws (`bind`).
    bound: [*mut SDL_GPUTexture; 3],
    /// The width and height of each plane texture.
    sizes: [(u32, u32); 3],
    sample_bytes: usize,
    /// The decoder's images wrapped as textures of this device, per plane set, for the planes
    /// they belong to.
    wrapped: Option<Wrapped>,
    /// The set the shown picture is sampled from, in place (`PlaneHandoff::Sample`).
    handover: Handover<Lease>,
    /// The render states drawing the wrapped images of VA-API pictures sampled in place
    /// (`vulkan_video::Planes::sample`), by their luma texture, made on first use, with the
    /// textures they draw. The textures belong to the presenter's planes, which outlive the pass.
    sampled_states: std::collections::HashMap<usize, (*mut SDL_GPURenderState, [*mut SDL_GPUTexture; 3])>,
    /// The state of the picture sampled last, which the next draw uses; null: the planes' copies.
    sampled_active: *mut SDL_GPURenderState,
    /// Frame Warp's HUD mask (`hudMask`, the fourth sampler slot), `MASK_ROW_BYTES` by
    /// `MASK_ROWS` R8 texels, made with the first mask a draw shifts with (`upload_mask`);
    /// null until then, the luma plane standing in.
    mask: *mut SDL_GPUTexture,
    /// The HUD the mask texture holds.
    mask_made: Option<Arc<super::frame_warp::Hud>>,
    /// A mask could not be made or uploaded (said once): the HUD moves with the picture.
    mask_failed: bool,
}

/// The mask texture's size: a row of the widest mask's bytes (`frame_warp::mask_bytes`), and
/// its rows.
const MASK_ROW_BYTES: u32 = super::frame_warp::MASK_EDGE.div_ceil(32) as u32 * 4;
const MASK_ROWS: u32 = super::frame_warp::MASK_EDGE as u32;

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
            Some(&(state, _)) => state,
            None => {
                let textures = [textures[0], textures[1], textures[1]];
                let state = self.make_state(renderer, textures)?;
                self.sampled_states.insert(key, (state, textures));
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
            transfer: ptr::null_mut(), state: ptr::null_mut(), bound: [ptr::null_mut(); 3], sizes, sample_bytes, wrapped: None, handover: Handover::default(),
            sampled_states: Default::default(), sampled_active: ptr::null_mut(), mask: ptr::null_mut(), mask_made: None, mask_failed: false,
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
        // The shader reads the mask only while Frame Warp shifts with one, so its texture waits
        // for the first (`upload_mask`): until then the slot holds a plane.
        Ok(pass)
    }

    /// The render state drawing `textures` in the shader's three plane slots.
    unsafe fn bind(&mut self, renderer: *mut SDL_Renderer, textures: [*mut SDL_GPUTexture; 3]) -> Result<(), String> {
        self.state = self.make_state(renderer, textures)?;
        self.bound = textures;
        Ok(())
    }

    /// Makes every render state again, now that the mask texture exists for their fourth slot:
    /// those made before it hold a plane there. SDL flushes what was queued with a state before
    /// it lets the state go.
    unsafe fn rebind_mask(&mut self) -> Result<(), String> {
        let renderer = self.renderer;
        if !self.state.is_null() {
            let state = self.make_state(renderer, self.bound)?;
            SDL_DestroyGPURenderState(std::mem::replace(&mut self.state, state));
        }
        let sets = self.wrapped.as_ref().map(|wrapped| wrapped.sets.clone()).unwrap_or_default();
        for (set, textures) in sets.into_iter().enumerate() {
            let Some(old) = self.wrapped.as_ref().and_then(|wrapped| wrapped.states.get(set).copied()) else { break };
            if old.is_null() {
                continue;
            }
            let state = self.make_state(renderer, textures)?;
            if let Some(wrapped) = self.wrapped.as_mut() {
                wrapped.states[set] = state;
            }
            SDL_DestroyGPURenderState(old);
        }
        let sampled: Vec<_> = self.sampled_states.iter().map(|(&key, &made)| (key, made)).collect();
        for (key, (old, textures)) in sampled {
            let state = self.make_state(renderer, textures)?;
            self.sampled_states.insert(key, (state, textures));
            if self.sampled_active == old {
                self.sampled_active = state;
            }
            SDL_DestroyGPURenderState(old);
        }
        Ok(())
    }

    unsafe fn make_state(&self, renderer: *mut SDL_Renderer, textures: [*mut SDL_GPUTexture; 3]) -> Result<*mut SDL_GPURenderState, String> {
        // The fourth slot is Frame Warp's HUD mask; the shader loads it, so the sampler is any.
        let mask = if self.mask.is_null() { textures[0] } else { self.mask };
        let bindings = [textures[0], textures[1], textures[2], mask].map(|texture| SDL_GPUTextureSamplerBinding { texture, sampler: self.sampler });
        let state = SDL_CreateGPURenderState(renderer, &SDL_GPURenderStateCreateInfo {
            fragment_shader: self.shader, num_sampler_bindings: 4, sampler_bindings: bindings.as_ptr(), ..Default::default() });
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
    #[cfg(test)]
    pub unsafe fn draw_direct(&mut self, renderer: *mut SDL_Renderer, parameters: &[u32; 8], destination: &SDL_FRect, visible: (i32, i32), look: SdlLook) -> Result<(), String> {
        self.draw_direct_warped(renderer, parameters, destination, visible, look, &super::frame_warp::WarpDraw::default())
    }

    /// `draw_direct` with the picture shifted by Frame Warp's `warp`, its HUD mask uploaded when
    /// it changed.
    ///
    /// # Safety
    /// As `draw_direct`.
    pub unsafe fn draw_direct_warped(&mut self, renderer: *mut SDL_Renderer, parameters: &[u32; 8], destination: &SDL_FRect, visible: (i32, i32), look: SdlLook,
        warp: &super::frame_warp::WarpDraw) -> Result<(), String> {
        let mut viewport = SDL_Rect::default();
        SDL_GetRenderViewport(renderer, &mut viewport);
        let mut uniforms = direct_uniforms(parameters, destination, (viewport.x, viewport.y), visible, look);
        if !warp.is_identity() {
            let mask = self.ready_mask(renderer, warp);
            uniforms.warp = warp.row(mask);
            uniforms.warp_lens = warp.lens_row();
        }
        self.set_uniforms(&uniforms)?;
        let drawn = SDL_SetGPURenderState(renderer, self.active_state()) && SDL_RenderFillRect(renderer, destination);
        let error = if drawn { String::new() } else { crate::sdl::error() };
        SDL_SetGPURenderState(renderer, ptr::null_mut());
        if drawn { Ok(()) } else { Err(format!("the planes cannot be drawn into the back buffer: {error}")) }
    }

    /// Whether the mask texture holds `warp`'s HUD mask for the shader, uploading it when it
    /// changed. A mask that cannot be made or uploaded is said once and drawn as none from then
    /// on: the HUD moves with the picture.
    unsafe fn ready_mask(&mut self, renderer: *mut SDL_Renderer, warp: &super::frame_warp::WarpDraw) -> bool {
        let Some(hud) = warp.mask() else { return false };
        if self.mask_failed { return false; }
        if self.mask_made.as_ref().is_some_and(|made| Arc::ptr_eq(made, hud)) { return true; }
        match self.upload_mask(renderer, &hud.mask) {
            Ok(()) => {
                self.mask_made = Some(hud.clone());
                true
            }
            Err(error) => {
                eprintln!("nightjar: Frame Warp: the HUD mask cannot reach the GPU ({error}); the HUD moves with the picture");
                self.mask_failed = true;
                false
            }
        }
    }

    /// Uploads `mask` (`frame_warp::mask_bytes`) into the top left of the mask texture, ahead of
    /// the renderer's next draws. The first upload makes the texture and binds it.
    unsafe fn upload_mask(&mut self, renderer: *mut SDL_Renderer, mask: &nightjar_core::frame_warp::Mask) -> Result<(), String> {
        let (row, bytes) = super::frame_warp::mask_bytes(mask);
        let rows = u32::from(mask.height);
        if bytes.is_empty() || row as u32 > MASK_ROW_BYTES || rows > MASK_ROWS {
            return Err(format!("a {}x{} mask is larger than its texture", mask.width, mask.height));
        }
        if self.mask.is_null() {
            self.mask = SDL_CreateGPUTexture(self.device, &SDL_GPUTextureCreateInfo {
                r#type: SDL_GPU_TEXTURETYPE_2D, format: SDL_GPU_TEXTUREFORMAT_R8_UNORM, usage: SDL_GPU_TEXTUREUSAGE_SAMPLER,
                width: MASK_ROW_BYTES, height: MASK_ROWS, layer_count_or_depth: 1, num_levels: 1, sample_count: SDL_GPU_SAMPLECOUNT_1, ..Default::default() });
            if self.mask.is_null() {
                return Err(format!("its texture could not be made: {}", crate::sdl::error()));
            }
            self.rebind_mask().map_err(|error| format!("its texture cannot be bound: {error}"))?;
        }
        // The draws queued so far read the mask before this upload replaces it.
        if !SDL_FlushRenderer(renderer) {
            return Err(format!("the renderer cannot flush: {}", crate::sdl::error()));
        }
        let transfer = SDL_CreateGPUTransferBuffer(self.device, &SDL_GPUTransferBufferCreateInfo {
            usage: SDL_GPU_TRANSFERBUFFERUSAGE_UPLOAD, size: bytes.len() as u32, ..Default::default() });
        if transfer.is_null() {
            return Err(format!("no transfer buffer: {}", crate::sdl::error()));
        }
        let mapped = SDL_MapGPUTransferBuffer(self.device, transfer, false).cast::<u8>();
        let result = if mapped.is_null() {
            Err(format!("the transfer buffer cannot be mapped: {}", crate::sdl::error()))
        } else {
            ptr::copy_nonoverlapping(bytes.as_ptr(), mapped, bytes.len());
            SDL_UnmapGPUTransferBuffer(self.device, transfer);
            let commands = SDL_AcquireGPUCommandBuffer(self.device);
            if commands.is_null() {
                Err(format!("no GPU command buffer: {}", crate::sdl::error()))
            } else {
                let copy = SDL_BeginGPUCopyPass(commands);
                SDL_UploadToGPUTexture(copy,
                    &SDL_GPUTextureTransferInfo { transfer_buffer: transfer, offset: 0, pixels_per_row: row as u32, rows_per_layer: rows },
                    &SDL_GPUTextureRegion { texture: self.mask, w: row as u32, h: rows, d: 1, ..Default::default() },
                    true);
                SDL_EndGPUCopyPass(copy);
                if SDL_SubmitGPUCommandBuffer(commands) { Ok(()) } else { Err(format!("the upload cannot be submitted: {}", crate::sdl::error())) }
            }
        };
        // SDL keeps the buffer until the GPU is done with it.
        SDL_ReleaseGPUTransferBuffer(self.device, transfer);
        result
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
    /// What flag 512 reads of Frame Warp: `WarpDraw::row`; all zero draws the picture in place.
    pub warp: [f32; 4],
    /// Its lens terms (`WarpDraw::lens_row`), read only beside a row that shifts.
    pub warp_lens: [f32; 4],
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
        warp: [0.0; 4],
        warp_lens: nightjar_core::frame_warp::FLAT,
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
                    for (target, source) in target.as_chunks_mut::<2>().0.iter_mut().zip(source.as_chunks::<2>().0) {
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
            target.as_chunks_mut::<2>().0.iter_mut().zip(source.as_chunks::<2>().0).for_each(|(target, source)| put(target, source));
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
                for (_, (state, _)) in self.sampled_states.drain() { SDL_DestroyGPURenderState(state); }
            }
            if !self.state.is_null() { SDL_DestroyGPURenderState(self.state); }
            if !self.transfer.is_null() { SDL_ReleaseGPUTransferBuffer(self.device, self.transfer); }
            for texture in self.textures { if !texture.is_null() { SDL_ReleaseGPUTexture(self.device, texture); } }
            if !self.mask.is_null() { SDL_ReleaseGPUTexture(self.device, self.mask); }
            if !self.sampler.is_null() { SDL_ReleaseGPUSampler(self.device, self.sampler); }
            if !self.shader.is_null() { SDL_ReleaseGPUShader(self.device, self.shader); }
        }
    }
}

#[cfg(test)]
mod tests;
