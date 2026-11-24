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

mod fill;
use fill::{fill_planes, fill_semi_planar, semi_planar};
#[cfg(test)]
use fill::high_bit_planes;

mod direct;
pub use direct::{SdlLook, Uniforms};
#[cfg(test)]
use direct::{direct_uniforms, DIRECT_FLAG, GAMMA22_TARGET_FLAG, MASK_ROW_BYTES, PQ_TARGET_FLAG, SRGB_TARGET_FLAG};

fn device_of(renderer: *mut SDL_Renderer) -> *mut SDL_GPUDevice {
    crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_GPU_DEVICE_POINTER).cast()
}

fn plane_format(ten_bit: bool) -> SDL_GPUTextureFormat {
    if ten_bit { SDL_GPU_TEXTUREFORMAT_R16_UNORM } else { SDL_GPU_TEXTUREFORMAT_R8_UNORM }
}

fn chroma_format(ten_bit: bool) -> SDL_GPUTextureFormat {
    if ten_bit { SDL_GPU_TEXTUREFORMAT_R16G16_UNORM } else { SDL_GPU_TEXTUREFORMAT_R8G8_UNORM }
}

/// # Safety
/// `device` must be a live GPU device of the calling thread's renderer.
unsafe fn create_shader(device: *mut SDL_GPUDevice) -> *mut SDL_GPUShader {
    // SAFETY: Caller guarantees `device` is a live GPU device on its renderer's thread.
    unsafe { SDL_CreateGPUShader(device, &super::shader::Shader::Planes.info()) }
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
    /// Two-plane NV12/P010 upload (`R8`/`R16` luma + `R8G8`/`R16G16` interleaved chroma, shader flag 256).
    semi_planar: bool,
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
    decode_value: u64,
}

impl SetGpu for Sets<'_> {
    fn flush(&mut self) -> Result<(), String> {
        // SAFETY: The pass's live renderer, on its thread.
        if unsafe { SDL_FlushRenderer(self.renderer) } { Ok(()) } else { Err(format!("the renderer cannot flush: {}", crate::sdl::error())) }
    }

    fn sampled(&mut self, set: usize) -> Result<(), String> {
        self.images.sampled(set, self.decode_value)
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
        // SAFETY: Caller guarantees `device` is a live SDL_GPUDevice on its own thread.
        unsafe {
            if !SDL_GPUTextureSupportsFormat(device, plane_format(ten_bit), SDL_GPU_TEXTURETYPE_2D, SDL_GPU_TEXTUREUSAGE_SAMPLER) {
                return Err(format!("the GPU renderer cannot sample {} planes", if ten_bit { "R16" } else { "R8" }));
            }
            let shader = create_shader(device);
            if shader.is_null() {
                return Err(format!("the PyroWave plane shader does not load: {}", crate::sdl::error()));
            }
            SDL_ReleaseGPUShader(device, shader);
        }
        Ok(())
    }

    /// The pass for planes shaped like `config`, drawing on `renderer`'s GPU device.
    ///
    /// # Safety
    /// `renderer` must be the live SDL GPU renderer of the calling thread.
    pub unsafe fn new(renderer: *mut SDL_Renderer, config: GpuConfig) -> Result<Self, String> {
        // SAFETY: Caller guarantees `renderer` is live on the calling thread.
        unsafe { Self::with_planes(renderer, [0, 1, 2].map(|plane| config.plane_size(plane)), config.ten_bit) }
    }

    /// The pass for the decoder's `planes`: one without plane textures of its own when it samples
    /// the decoder's images in place, else `new`.
    ///
    /// # Safety
    /// As `new`.
    pub unsafe fn for_planes(renderer: *mut SDL_Renderer, planes: &Planes) -> Result<Self, String> {
        let config = planes.config();
        // SAFETY: Caller guarantees `renderer` is live on the calling thread.
        unsafe {
            if !samples_in_place(planes) {
                return Self::new(renderer, config);
            }
            Self::shader_only(renderer, [0, 1, 2].map(|plane| config.plane_size(plane)), if config.ten_bit { 2 } else { 1 })
        }
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
        // SAFETY: Caller guarantees `renderer` is live on the calling thread.
        unsafe { Self::with_planes(renderer, [0, 1, 2].map(|plane| layout.plane_size(plane)), layout.ten_bit) }
    }

    /// The pass for semi-planar NV12/P010 pictures: uploads luma (`R8`/`R16`) and interleaved
    /// chroma (`R8G8`/`R16G16`) directly without CPU shifts or UV deinterleaving (shader flag 256).
    ///
    /// # Safety
    /// `renderer` must be the live SDL GPU renderer of the calling thread.
    pub unsafe fn for_semi_planar(renderer: *mut SDL_Renderer, layout: crate::media::PlanarLayout) -> Result<Self, String> {
        let sizes = [layout.plane_size(0), layout.plane_size(1), (0, 0)];
        // SAFETY: Caller guarantees `renderer` is live on the calling thread.
        unsafe {
            let mut pass = Self::shader_only(renderer, sizes, if layout.ten_bit { 2 } else { 1 })?;
            pass.semi_planar = true;
            let device = pass.device;
            for (plane, format) in [plane_format(layout.ten_bit), chroma_format(layout.ten_bit)].into_iter().enumerate() {
                let (width, height) = sizes[plane];
                pass.textures[plane] = SDL_CreateGPUTexture(device, &SDL_GPUTextureCreateInfo {
                    r#type: SDL_GPU_TEXTURETYPE_2D, format, usage: SDL_GPU_TEXTUREUSAGE_SAMPLER,
                    width, height, layer_count_or_depth: 1, num_levels: 1, sample_count: SDL_GPU_SAMPLECOUNT_1, ..Default::default() });
                if pass.textures[plane].is_null() {
                    return Err(format!("a {width}x{height} semi-planar texture cannot be created: {}", crate::sdl::error()));
                }
            }
            let bytes = pass.plane_bytes(0) + pass.plane_bytes(1);
            pass.transfer = SDL_CreateGPUTransferBuffer(device, &SDL_GPUTransferBufferCreateInfo {
                usage: SDL_GPU_TRANSFERBUFFERUSAGE_UPLOAD, size: bytes as u32, ..Default::default() });
            if pass.transfer.is_null() {
                return Err(format!("the plane transfer buffer cannot be created: {}", crate::sdl::error()));
            }
            pass.bind(renderer, [pass.textures[0], pass.textures[1], pass.textures[1]])?;
            Ok(pass)
        }
    }

    /// The pass for Vulkan video pictures: it draws `textures`, the luma and the interleaved chroma
    /// of `vulkan_video::Planes`, which the presenter copies each picture into on the GPU, with
    /// flag 256 (`draw_bound`); it has no textures or upload of its own.
    ///
    /// # Safety
    /// `renderer` must be the live SDL GPU renderer of the calling thread, and `textures` live
    /// textures of its device that outlive the pass.
    pub unsafe fn for_vulkan(renderer: *mut SDL_Renderer, textures: [*mut SDL_GPUTexture; 2]) -> Result<Self, String> {
        // SAFETY: Caller guarantees `renderer` and `textures` are live and `textures` outlive the pass.
        unsafe {
            let mut pass = Self::shader_only(renderer, [(0, 0); 3], 1)?;
            pass.bind(renderer, [textures[0], textures[1], textures[1]])?;
            Ok(pass)
        }
    }

    /// Draws the textures a `for_vulkan` pass was made with, converted, over all of `target`
    /// (null: nothing yet, `draw_direct` draws them later).
    ///
    /// # Safety
    /// `renderer` and `target` must be live and the pass the one made on this renderer.
    pub unsafe fn draw_bound(&mut self, renderer: *mut SDL_Renderer, target: *mut SDL_Texture, parameters: &[u32; 8]) -> Result<(), String> {
        // SAFETY: Caller guarantees `renderer` and `target` are live and match this pass.
        unsafe { self.convert(renderer, target, parameters, None) }
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
        // SAFETY: Caller guarantees `renderer`, `target`, and `textures` are live and outlive the pass.
        unsafe {
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
    }

    unsafe fn with_planes(renderer: *mut SDL_Renderer, sizes: [(u32, u32); 3], ten_bit: bool) -> Result<Self, String> {
        // SAFETY: Caller guarantees `renderer` is live on the calling thread.
        unsafe {
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
    }

    /// The pass's shader and sampler, without textures or a render state yet.
    unsafe fn shader_only(renderer: *mut SDL_Renderer, sizes: [(u32, u32); 3], sample_bytes: usize) -> Result<Self, String> {
        let device = device_of(renderer);
        if device.is_null() {
            return Err("it needs the SDL GPU renderer".into());
        }
        let mut pass = Self {
            renderer, device, shader: ptr::null_mut(), sampler: ptr::null_mut(), textures: [ptr::null_mut(); 3],
            transfer: ptr::null_mut(), state: ptr::null_mut(), bound: [ptr::null_mut(); 3], sizes, sample_bytes, semi_planar: false,
            wrapped: None, handover: Handover::default(), sampled_states: Default::default(), sampled_active: ptr::null_mut(),
            mask: ptr::null_mut(), mask_made: None, mask_failed: false,
        };
        // SAFETY: `device` is the live GPU device of `renderer` on the calling thread.
        unsafe {
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
        }
        // The shader reads the mask only while Frame Warp shifts with one, so its texture waits
        // for the first (`upload_mask`): until then the slot holds a plane.
        Ok(pass)
    }

    /// The render state drawing `textures` in the shader's three plane slots.
    unsafe fn bind(&mut self, renderer: *mut SDL_Renderer, textures: [*mut SDL_GPUTexture; 3]) -> Result<(), String> {
        // SAFETY: Caller guarantees `renderer` and `textures` are live on the calling thread.
        self.state = unsafe { self.make_state(renderer, textures) }?;
        self.bound = textures;
        Ok(())
    }

    /// Makes every render state again, now that the mask texture exists for their fourth slot:
    /// those made before it hold a plane there. SDL flushes what was queued with a state before
    /// it lets the state go.
    unsafe fn rebind_mask(&mut self) -> Result<(), String> {
        let renderer = self.renderer;
        // SAFETY: `renderer` and its render states are live on the calling thread.
        unsafe {
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
        }
        Ok(())
    }

    unsafe fn make_state(&self, renderer: *mut SDL_Renderer, textures: [*mut SDL_GPUTexture; 3]) -> Result<*mut SDL_GPURenderState, String> {
        // The fourth slot is Frame Warp's HUD mask; the shader loads it, so the sampler is any.
        let mask = if self.mask.is_null() { textures[0] } else { self.mask };
        let bindings = [textures[0], textures[1], textures[2], mask].map(|texture| SDL_GPUTextureSamplerBinding { texture, sampler: self.sampler });
        // SAFETY: Caller guarantees `renderer` and `textures` are live on the calling thread.
        let state = unsafe {
            SDL_CreateGPURenderState(renderer, &SDL_GPURenderStateCreateInfo {
                fragment_shader: self.shader, num_sampler_bindings: 4, sampler_bindings: bindings.as_ptr(), ..Default::default() })
        };
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
    pub unsafe fn draw(&mut self, renderer: *mut SDL_Renderer, target: *mut SDL_Texture, lease: &Lease, decode_value: u64, parameters: &[u32; 8], stages: &mut Stages<2>) -> Result<(), String> {
        if let Some(images) = lease.planes().images() {
            let images = images.clone();
            if !self.suits(lease.planes()) {
                return Err("the plane pass was made for the other way of reading the decoder's images".into());
            }
            // SAFETY: Caller guarantees `renderer` and `target` are live and match this pass.
            return unsafe {
                if images.handoff() == PlaneHandoff::Sample {
                    self.draw_sampled(renderer, target, lease, decode_value, &images, parameters, stages)
                } else {
                    self.draw_shared(renderer, target, lease, decode_value, &images, parameters, stages)
                }
            };
        }
        if self.textures[0].is_null() {
            return Err("the plane pass was made for the decoder's images, not CPU planes".into());
        }
        let bytes = [0, 1, 2].map(|plane| self.plane_bytes(plane));
        let fill = |mapped: *mut u8, offsets: [usize; 3]| {
            lease.with_buffers(|buffers| {
                for (plane, buffer) in buffers.iter().enumerate() {
                    // SAFETY: `mapped` holds at least `offsets[plane] + bytes[plane]` bytes and `buffer` holds `bytes[plane]`.
                    unsafe { ptr::copy_nonoverlapping(buffer.as_ptr(), mapped.add(offsets[plane]), bytes[plane]); }
                }
            });
        };
        // SAFETY: Caller guarantees `renderer` and `target` are live; `draw_with` maps `transfer` for `bytes`.
        unsafe { self.draw_with(renderer, target, parameters, Some(stages), fill) }
    }

    /// `draw` for a set in the decoder's images: copies it into the plane textures on the GPU,
    /// signals the reuse timeline behind the copy and gives the set back with that value, so the
    /// decoder can write it again as soon as the copy is done, not after the draw.
    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_shared(&mut self, renderer: *mut SDL_Renderer, target: *mut SDL_Texture, lease: &Lease, decode_value: u64, images: &SharedImages, parameters: &[u32; 8], stages: &mut Stages<2>) -> Result<(), String> {
        let step = Instant::now();
        // SAFETY: Caller guarantees `renderer` and `target` are live on the calling thread.
        unsafe {
            if decode_value > 0 {
                images.wait_decode(decode_value, std::time::Duration::from_secs(2))?;
            }
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
    }

    /// `draw` for a set sampled in place: takes the picture's lease over, makes the set readable
    /// and gives the set it replaces back behind the draws recorded so far (`Handover::show`),
    /// then draws from it (now, into `target`, or at present time).
    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_sampled(&mut self, renderer: *mut SDL_Renderer, target: *mut SDL_Texture, lease: &Lease, decode_value: u64, images: &Arc<SharedImages>, parameters: &[u32; 8], stages: &mut Stages<2>) -> Result<(), String> {
        let step = Instant::now();
        // SAFETY: Caller guarantees `renderer` and `target` are live on the calling thread.
        unsafe {
            self.wrap(lease.planes(), images)?;
            let next = lease.hand_over().ok_or("the picture's plane set was given back already")?;
            self.handover.show(&mut Sets { renderer, images: images.as_ref(), decode_value }, next)?;
            stages.add(super::pyrowave_gpu::UPLOAD, step);
            self.convert(renderer, target, parameters, Some(stages))
        }
    }

    /// The textures wrapping each plane set of `planes`' images (and, sampled in place, a render
    /// state per set), made on first use. Those of earlier planes (an earlier decoder's) are
    /// released first, the set held from them given back.
    unsafe fn wrap(&mut self, planes: &Arc<Planes>, images: &SharedImages) -> Result<&Wrapped, String> {
        // SAFETY: `self.renderer`, `self.device`, and `images` are live on the calling thread.
        unsafe {
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
                            if sampled && let Some(wrapped) = self.wrapped.as_mut() { wrapped.states.push(state); }
                        }
                        outcome => {
                            let error = outcome.err().unwrap_or_else(crate::sdl::error);
                            self.unwrap();
                            return Err(format!("the decoder's plane images cannot be wrapped as textures: {error}"));
                        }
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
        self.handover.finish(&mut Sets { renderer: self.renderer, images: images.as_ref(), decode_value: 0 });
    }

    /// Gives the held set back, releases the wrapped textures and their render states and lets
    /// the images go. SDL destroys a texture's views only once its last command is done, and the
    /// images must outlive them, so this waits for that.
    unsafe fn unwrap(&mut self) {
        // SAFETY: `self.renderer` and `self.device` outlive the wrapped resources on this thread.
        unsafe {
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
        let (sizes, sample_bytes, fast_semi) = (self.sizes, self.sample_bytes, self.semi_planar);
        let fill = |mapped, offsets: [usize; 3]| {
            // SAFETY: `info` matches `sizes` and `mapped` holds the packed planes at `offsets`.
            unsafe {
                if fast_semi {
                    fill_semi_planar(info, [sizes[0], sizes[1]], sample_bytes, mapped, [offsets[0], offsets[1]]);
                } else {
                    fill_planes(info, sizes, sample_bytes, mapped, offsets);
                }
            }
        };
        // SAFETY: Caller guarantees `renderer`, `target`, and `info` are live and match `sizes`.
        unsafe { self.draw_with(renderer, target, parameters, None, fill) }
    }

    /// The bytes of plane `plane` in the transfer buffer (tightly packed rows).
    fn plane_bytes(&self, plane: usize) -> usize {
        let (width, height) = self.sizes[plane];
        let channels = if self.semi_planar && plane == 1 { 2 } else { 1 };
        width as usize * height as usize * self.sample_bytes * channels
    }

    /// Fills the transfer buffer with `fill` (given the mapped buffer and each plane's offset in
    /// it), uploads it into the plane textures and draws them over `target`.
    unsafe fn draw_with(&mut self, renderer: *mut SDL_Renderer, target: *mut SDL_Texture, parameters: &[u32; 8], mut stages: Option<&mut Stages<2>>, fill: impl FnOnce(*mut u8, [usize; 3])) -> Result<(), String> {
        let step = Instant::now();
        // SAFETY: Caller guarantees `renderer` and `target` are live on the calling thread.
        unsafe {
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
            let count = if self.semi_planar { 2 } else { 3 };
            for (plane, offset) in offsets.into_iter().enumerate().take(count) {
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
    }

    /// Draws the plane textures, converted, over `target`. A null `target` leaves the planes
    /// copied and undrawn, for `draw_direct` (or a later `draw_bound`) to draw.
    unsafe fn convert(&mut self, renderer: *mut SDL_Renderer, target: *mut SDL_Texture, parameters: &[u32; 8], stages: Option<&mut Stages<2>>) -> Result<(), String> {
        if target.is_null() {
            return Ok(());
        }
        let step = Instant::now();
        // SAFETY: Caller guarantees `renderer` and `target` are live on the calling thread.
        unsafe {
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
}

/// Whether the presenter samples `planes` in place (images on the renderer's device, not copied).
fn samples_in_place(planes: &Planes) -> bool {
    planes.images().is_some_and(|images| images.handoff() == PlaneHandoff::Sample)
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
