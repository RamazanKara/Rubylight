//! Draws the GPU decoder's planes through SDL's GPU renderer (brief G3, Linux).
//!
//! The planes the SDK decoded sit in CPU buffers (`gpu_planes::Lease`). This pass copies them into
//! three persistent R8 or R16 UNORM textures of the renderer's device and draws a full-target
//! rectangle with `pyrowave-planes.spv`, the `planes` entry of pyrowave.hlsl, which converts the
//! planes to the same linear RGB as the CPU path. Nothing here waits for the GPU: the uploads and
//! the draw are ordered by the renderer's own queue.

use std::ptr;
use std::time::Instant;

use sdl3_sys::everything::*;

use super::gpu_planes::Lease;
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
    config: GpuConfig,
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
        let device = device_of(renderer);
        if device.is_null() {
            return Err("it needs the SDL GPU renderer".into());
        }
        let mut pass = Self {
            device, shader: ptr::null_mut(), sampler: ptr::null_mut(), textures: [ptr::null_mut(); 3],
            transfer: ptr::null_mut(), state: ptr::null_mut(), config,
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
        for (plane, texture) in pass.textures.iter_mut().enumerate() {
            let (width, height) = config.plane_size(plane);
            *texture = SDL_CreateGPUTexture(device, &SDL_GPUTextureCreateInfo {
                r#type: SDL_GPU_TEXTURETYPE_2D, format: plane_format(config.ten_bit), usage: SDL_GPU_TEXTUREUSAGE_SAMPLER,
                width, height, layer_count_or_depth: 1, num_levels: 1, sample_count: SDL_GPU_SAMPLECOUNT_1, ..Default::default() });
            if texture.is_null() {
                return Err(format!("a {width}x{height} plane texture cannot be created: {}", crate::sdl::error()));
            }
        }
        let bytes: usize = (0..3).map(|plane| config.plane_bytes(plane)).sum();
        pass.transfer = SDL_CreateGPUTransferBuffer(device, &SDL_GPUTransferBufferCreateInfo {
            usage: SDL_GPU_TRANSFERBUFFERUSAGE_UPLOAD, size: bytes as u32, ..Default::default() });
        if pass.transfer.is_null() {
            return Err(format!("the plane transfer buffer cannot be created: {}", crate::sdl::error()));
        }
        let bindings = pass.textures.map(|texture| SDL_GPUTextureSamplerBinding { texture, sampler: pass.sampler });
        pass.state = SDL_CreateGPURenderState(renderer, &SDL_GPURenderStateCreateInfo {
            fragment_shader: pass.shader, num_sampler_bindings: 3, sampler_bindings: bindings.as_ptr(), ..Default::default() });
        if pass.state.is_null() {
            return Err(format!("the plane render state cannot be created: {}", crate::sdl::error()));
        }
        Ok(pass)
    }

    /// Uploads the planes of `lease` and draws them, converted, over all of `target` (the
    /// renderer's RGBA64F picture texture). `parameters` are the shader's constants. The CPU time
    /// of copying and submitting the upload goes to `stages`' upload, that of the conversion to
    /// its passes (the GPU runs on after the submit returns).
    ///
    /// # Safety
    /// `renderer` and `target` must be live and the pass the one made on this renderer.
    pub unsafe fn draw(&mut self, renderer: *mut SDL_Renderer, target: *mut SDL_Texture, lease: &Lease, parameters: &[u32; 8], stages: &mut Stages<2>) -> Result<(), String> {
        let step = Instant::now();
        if !SDL_FlushRenderer(renderer) {
            return Err(format!("the renderer cannot flush: {}", crate::sdl::error()));
        }
        let config = self.config;
        let mapped = SDL_MapGPUTransferBuffer(self.device, self.transfer, true).cast::<u8>();
        if mapped.is_null() {
            return Err(format!("the plane transfer buffer cannot be mapped: {}", crate::sdl::error()));
        }
        let mut offsets = [0u32; 3];
        let mut offset = 0usize;
        lease.with_buffers(|buffers| {
            for (plane, buffer) in buffers.iter().enumerate() {
                let bytes = config.plane_bytes(plane);
                ptr::copy_nonoverlapping(buffer.as_ptr(), mapped.add(offset), bytes);
                offsets[plane] = offset as u32;
                offset += bytes;
            }
        });
        SDL_UnmapGPUTransferBuffer(self.device, self.transfer);
        let commands = SDL_AcquireGPUCommandBuffer(self.device);
        if commands.is_null() {
            return Err(format!("no GPU command buffer for the plane upload: {}", crate::sdl::error()));
        }
        let copy = SDL_BeginGPUCopyPass(commands);
        for (plane, offset) in offsets.into_iter().enumerate() {
            let (width, height) = config.plane_size(plane);
            SDL_UploadToGPUTexture(copy,
                &SDL_GPUTextureTransferInfo { transfer_buffer: self.transfer, offset, pixels_per_row: width, rows_per_layer: height },
                &SDL_GPUTextureRegion { texture: self.textures[plane], w: width, h: height, d: 1, ..Default::default() },
                true);
        }
        SDL_EndGPUCopyPass(copy);
        if !SDL_SubmitGPUCommandBuffer(commands) {
            return Err(format!("the plane upload cannot be submitted: {}", crate::sdl::error()));
        }
        stages.add(super::pyrowave_gpu::UPLOAD, step);
        let step = Instant::now();
        if !SDL_SetGPURenderStateFragmentUniforms(self.state, 0, parameters.as_ptr().cast(), 32) {
            return Err(format!("the plane constants cannot be set: {}", crate::sdl::error()));
        }
        let previous = SDL_GetRenderTarget(renderer);
        if !SDL_SetRenderTarget(renderer, target) {
            return Err(format!("the picture texture cannot be the render target: {}", crate::sdl::error()));
        }
        let drawn = SDL_SetGPURenderState(renderer, self.state) && SDL_RenderFillRect(renderer, ptr::null()) && SDL_FlushRenderer(renderer);
        let error = if drawn { String::new() } else { crate::sdl::error() };
        SDL_SetGPURenderState(renderer, ptr::null_mut());
        SDL_SetRenderTarget(renderer, previous);
        stages.add(super::pyrowave_gpu::PASSES, step);
        if drawn { Ok(()) } else { Err(format!("the planes cannot be drawn: {error}")) }
    }
}

impl Drop for PlanesPass {
    fn drop(&mut self) {
        // SAFETY: These objects belong to the renderer's device, which outlives the pass.
        unsafe {
            if !self.state.is_null() { SDL_DestroyGPURenderState(self.state); }
            if !self.transfer.is_null() { SDL_ReleaseGPUTransferBuffer(self.device, self.transfer); }
            for texture in self.textures { if !texture.is_null() { SDL_ReleaseGPUTexture(self.device, texture); } }
            if !self.sampler.is_null() { SDL_ReleaseGPUSampler(self.device, self.sampler); }
            if !self.shader.is_null() { SDL_ReleaseGPUShader(self.device, self.shader); }
        }
    }
}
