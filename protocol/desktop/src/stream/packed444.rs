use crate::media;
use sdl3_sys::everything::*;
use std::ptr;

pub fn probe(renderer: *mut SDL_Renderer, hardware: media::Hardware) -> Result<u8, String> {
    if hardware == media::Hardware::None {
        return Err("Packed full chroma requires hardware decoding".into());
    }
    let fixtures: [(media::Codec, &[u8]); 4] = [
        (
            media::Codec::Hevc,
            include_bytes!("../../../../app/src/main/assets/packed444/hevc-8.bin"),
        ),
        (
            media::Codec::Hevc,
            include_bytes!("../../../../app/src/main/assets/packed444/hevc-10.bin"),
        ),
        (
            media::Codec::Av1,
            include_bytes!("../../../../app/src/main/assets/packed444/av1-8.bin"),
        ),
        (
            media::Codec::Av1,
            include_bytes!("../../../../app/src/main/assets/packed444/av1-10.bin"),
        ),
    ];
    let mut modes = 0;
    let mut failures = Vec::new();
    for (mode, (codec, data)) in fixtures.into_iter().enumerate() {
        match [1, 2].into_iter().try_for_each(|packing| verify(renderer, hardware, codec, data, mode % 2 != 0, packing)) {
            Ok(()) => modes |= 1 << mode,
            Err(reason) => {
                let failure = format!(
                    "{codec:?} {}-bit: {reason}",
                    if mode % 2 == 0 { 8 } else { 10 }
                );
                eprintln!("packed full chroma: {failure}");
                failures.push(failure);
            }
        }
    }
    if modes == 0 {
        Err(failures.join("; "))
    } else {
        Ok(modes)
    }
}

fn verify(
    renderer: *mut SDL_Renderer,
    hardware: media::Hardware,
    codec: media::Codec,
    data: &[u8],
    ten_bit: bool,
    packing: u32,
) -> Result<(), String> {
    // On the device and path a stream takes (`pipeline`): VAAPI on the GPU renderer's device with
    // its pictures mapped into Vulkan, or Vulkan video, is drawn from the planes; the rest from memory.
    let pictures = media::Pictures { ten_bit, chroma444: false, packed: true, waiting: 0, vaapi_node: media::vaapi_node(hardware) };
    let mut decoder = media::VideoDecoder::open_first(codec, pictures, hardware, true, |_, _| {})?;
    #[cfg(target_os = "linux")]
    if let media::Hardware::Auto(gpu, _) = hardware {
        if !gpu.is_null() {
            decoder.map_to_vulkan(gpu);
        }
    }
    let status = decoder.send(data, 0);
    if status < 0 {
        return Err(format!("decoder rejected the sampling fixture ({status})"));
    }
    let frame = match decoder.receive() {
        media::Received::Frame(frame) => frame,
        media::Received::None => return Err("decoder produced no fixture frame".into()),
        media::Received::Error => return Err("fixture decoding failed".into()),
    };
    #[cfg(target_os = "macos")]
    let frame = if frame.info().kind == media::FRAME_VIDEOTOOLBOX {
        frame.map_to_memory().ok_or("VideoToolbox could not map the packed picture")?
    } else { frame };
    let info = frame.info();
    if info.width != 128 || info.height != 128 || (info.ten_bit != 0) != ten_bit {
        return Err(format!(
            "decoded fixture has unexpected geometry or depth: {}x{}, ten bit={}",
            info.width, info.height, info.ten_bit
        ));
    }
    // SAFETY: The temporary target and readback surfaces belong to this renderer thread.
    unsafe {
        let target = SDL_CreateTexture(
            renderer,
            SDL_PIXELFORMAT_RGBA64_FLOAT,
            SDL_TEXTUREACCESS_TARGET,
            128,
            64,
        );
        if target.is_null() {
            return Err(format!(
                "float reconstruction target: {}",
                crate::sdl::error()
            ));
        }
        let parameters = [
            128,
            64,
            128,
            64,
            u32::from(ten_bit),
            1,
            1,
            u32::from(ten_bit),
            2.0f32.to_bits(),
            packing,
            0,
            0,
        ];
        #[cfg(unix)]
        let mapped = media::PlanarLayout::of_vulkan(&info).map(|layout| draw_mapped(renderer, target, &frame, layout, &parameters));
        #[cfg(not(unix))]
        let mapped: Option<Result<(Reconstruction, ()), String>> = None;
        let mut failed = None;
        let (reconstruction, mut valid, _planes) = match mapped {
            Some(Ok((reconstruction, planes))) => (Some(reconstruction), true, Some(planes)),
            Some(Err(error)) => { failed = Some(error); (None, false, None) }
            None => {
                let mut reconstruction = Reconstruction::new(renderer, target, &parameters);
                let valid = reconstruction.as_mut().is_some_and(|r| r.draw(renderer, target, &frame, &parameters));
                (reconstruction, valid, None)
            }
        };
        let mut reason = failed.unwrap_or_else(|| format!("GPU reconstruction unavailable: {}", crate::sdl::error()));
        let previous = SDL_GetRenderTarget(renderer);
        if valid && SDL_SetRenderTarget(renderer, target) {
            let surface = SDL_RenderReadPixels(renderer, ptr::null());
            valid = !surface.is_null();
            if valid {
                let pixels = SDL_ConvertSurface(surface, SDL_PIXELFORMAT_RGBA128_FLOAT);
                SDL_DestroySurface(surface);
                valid = !pixels.is_null();
                if valid {
                    let pixels_ref = &*pixels;
                    for y in 0..64 {
                        let row = std::slice::from_raw_parts(
                            (pixels_ref.pixels as *const u8)
                                .add(y * pixels_ref.pitch as usize)
                                .cast::<f32>(),
                            128 * 4,
                        );
                        for x in 0..128 {
                            let expected =
                                fixture_rgb(x, y, ten_bit, packing)
                                    .map(|v| if ten_bit { v / 2.0 } else { v });
                            let matches =
                                expected.iter().zip(&row[x * 4..x * 4 + 3]).all(|(&a, &b)| {
                                    b.is_finite() && (a - b).abs() < 0.002 + a.abs() * 0.004
                                });
                            if valid && !matches {
                                reason = format!("reconstruction differs at ({x}, {y}): expected {expected:?}, got {:?}", &row[x * 4..x * 4 + 3]);
                            }
                            valid &= matches;
                        }
                    }
                    SDL_DestroySurface(pixels);
                }
            }
        } else {
            valid = false;
        }
        SDL_SetRenderTarget(renderer, previous);
        drop(reconstruction);
        SDL_DestroyTexture(target);
        if valid {
            Ok(())
        } else {
            Err(reason)
        }
    }
}

/// Draws a Vulkan picture (mapped VAAPI or Vulkan video) into `target` from its planes, as the
/// presenter does; the reconstruction and planes stay alive until the target is read.
#[cfg(unix)]
unsafe fn draw_mapped(
    renderer: *mut SDL_Renderer,
    target: *mut SDL_Texture,
    frame: &media::Frame,
    layout: media::PlanarLayout,
    parameters: &[u32; 12],
) -> Result<(Reconstruction, crate::vulkan_video::Planes), String> {
    // SAFETY: The probe's live renderer on its thread, and a live Vulkan frame of a decoder on its
    // device (`media::vaapi_node`, `vulkan_video::device_context`); both outlive what is made here.
    unsafe {
        let device = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_GPU_DEVICE_POINTER);
        let mut planes = crate::vulkan_video::Planes::new(device.cast(), layout.width, layout.height, layout.ten_bit, layout.chroma444)
            .map_err(|error| format!("planes of the mapped picture: {error}"))?;
        let reconstruction = Reconstruction::for_planes(renderer, planes.textures())?;
        planes.copy(renderer, frame.raw.as_ptr()).map_err(|error| format!("copying the mapped picture: {error}"))?;
        if !reconstruction.draw_planes(renderer, target, parameters) {
            return Err(format!("GPU reconstruction from planes: {}", crate::sdl::error()));
        }
        Ok((reconstruction, planes))
    }
}

fn fixture_rgb(x: usize, y: usize, ten_bit: bool, packing: u32) -> [f32; 3] {
    let maximum = if ten_bit { 1023 } else { 255 };
    let luma = ((x + y * 13) & maximum) as f32;
    let (u, v) = if packing == 2 {
        let base = (x / 2 * 7 + y / 2 * 19 + 17) & maximum;
        let mut uv = [base, maximum - base];
        let detail = if !x.is_multiple_of(2) {
            Some([(x / 2 + (64 + y) * 13) & maximum, (64 + x / 2 + (64 + y) * 13) & maximum])
        } else if !y.is_multiple_of(2) {
            let u = (x / 2 * 7 + (32 + y / 2) * 19 + 17) & maximum;
            Some([u, maximum - u])
        } else { None };
        if let Some(detail) = detail {
            for plane in 0..2 {
                uv[plane] = (uv[plane] as i32 + 2 * detail[plane] as i32 - maximum as i32 - 1)
                    .clamp(0, maximum as i32) as usize;
            }
        }
        (uv[0], uv[1])
    } else if x.is_multiple_of(2) {
        let u = (x / 2 * 7 + y * 19 + 17) & maximum;
        (u, maximum - u)
    } else {
        (
            (x / 2 + (64 + y) * 13) & maximum,
            (64 + x / 2 + (64 + y) * 13) & maximum,
        )
    };
    let center = if ten_bit { 512.0 } else { 128.0 };
    let y = luma / maximum as f32;
    let u = (u as f32 - center) / maximum as f32;
    let v = (v as f32 - center) / maximum as f32;
    let (kr, kb) = if ten_bit {
        (0.2627, 0.0593)
    } else {
        (0.2126, 0.0722)
    };
    let r = y + 2.0 * (1.0 - kr) * v;
    let b = y + 2.0 * (1.0 - kb) * u;
    let g = (y - kr * r - kb * b) / (1.0 - kr - kb);
    let rgb = [r, g, b].map(|v: f32| {
        let v = v.clamp(0.0, 1.0);
        if ten_bit {
            let p = v.powf(1.0 / 78.84375);
            ((p - 3424.0 / 4096.0).max(0.0) / (2413.0 / 128.0 - 2392.0 / 128.0 * p).max(0.000001))
                .powf(16384.0 / 2610.0)
                * 125.0
        } else if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    });
    if ten_bit {
        [
            1.660491 * rgb[0] - 0.587641 * rgb[1] - 0.072850 * rgb[2],
            -0.124550 * rgb[0] + 1.1329 * rgb[1] - 0.008349 * rgb[2],
            -0.018151 * rgb[0] - 0.100579 * rgb[1] + 1.11873 * rgb[2],
        ]
    } else {
        rgb
    }
}

pub struct Reconstruction {
    #[cfg(windows)]
    native: Option<super::d3d11::Packed>,
    device: *mut SDL_GPUDevice,
    buffer: *mut SDL_GPUBuffer,
    transfer: *mut SDL_GPUTransferBuffer,
    state: *mut SDL_GPURenderState,
    shader: *mut SDL_GPUShader,
    /// The plane textures' sampler of a reconstruction from Vulkan planes (`for_planes`).
    sampler: *mut SDL_GPUSampler,
    bytes: u32,
}

impl Reconstruction {
    pub fn new(
        renderer: *mut SDL_Renderer,
        _target: *mut SDL_Texture,
        parameters: &[u32; 12],
    ) -> Option<Self> {
        let mut result = Self {
            #[cfg(windows)]
            native: None,
            device: ptr::null_mut(),
            buffer: ptr::null_mut(),
            transfer: ptr::null_mut(),
            state: ptr::null_mut(),
            shader: ptr::null_mut(),
            sampler: ptr::null_mut(),
            bytes: 0,
        };
        // SAFETY: Renderer and target are live and owned by the presenting thread.
        unsafe {
            #[cfg(windows)]
            {
                let device = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_D3D11_DEVICE_POINTER);
                if !device.is_null() {
                    let texture = if _target.is_null() { ptr::null_mut() } else {
                        SDL_GetPointerProperty(SDL_GetTextureProperties(_target), SDL_PROP_TEXTURE_D3D11_TEXTURE_POINTER, ptr::null_mut())
                    };
                    result.native = super::d3d11::Packed::new(
                        device,
                        texture,
                        parameters[2] as i32,
                        parameters[3] as i32 * 2,
                        parameters[4] != 0,
                        parameters[7] != 0,
                    ).map_err(|e| eprintln!("nightjar: packed reconstruction: {e}")).ok();
                    return result.native.is_some().then_some(result);
                }
            }
            result.device = SDL_GetPointerProperty(
                SDL_GetRendererProperties(renderer),
                SDL_PROP_RENDERER_GPU_DEVICE_POINTER,
                ptr::null_mut(),
            )
            .cast();
            if result.device.is_null() {
                return None;
            }
            result.shader = SDL_CreateGPUShader(result.device, &super::shader::Shader::Packed.info());
            result.bytes =
                parameters[2] * parameters[3] * 3 * if parameters[4] != 0 { 2 } else { 1 };
            result.buffer = SDL_CreateGPUBuffer(
                result.device,
                &SDL_GPUBufferCreateInfo {
                    usage: SDL_GPU_BUFFERUSAGE_GRAPHICS_STORAGE_READ,
                    size: result.bytes,
                    ..Default::default()
                },
            );
            result.transfer = SDL_CreateGPUTransferBuffer(
                result.device,
                &SDL_GPUTransferBufferCreateInfo {
                    usage: SDL_GPU_TRANSFERBUFFERUSAGE_UPLOAD,
                    size: result.bytes,
                    ..Default::default()
                },
            );
            if result.shader.is_null() || result.buffer.is_null() || result.transfer.is_null() {
                return None;
            }
            result.state = SDL_CreateGPURenderState(
                renderer,
                &SDL_GPURenderStateCreateInfo {
                    fragment_shader: result.shader,
                    num_storage_buffers: 1,
                    storage_buffers: &result.buffer,
                    ..Default::default()
                },
            );
            (!result.state.is_null()).then_some(result)
        }
    }

    /// The reconstruction from the Y and UV plane textures of a picture mapped into Vulkan
    /// (`vulkan_video::Planes::textures`, sized to the coded picture), with `packed444-planes.spv`:
    /// no picture crosses memory (#80: each 3840x4320 copy left the decoder 80 ms behind).
    ///
    /// # Safety
    /// `renderer` is the presenting thread's live GPU renderer, and `textures` live on its device
    /// until this is dropped.
    #[cfg(unix)]
    pub unsafe fn for_planes(renderer: *mut SDL_Renderer, textures: [*mut SDL_GPUTexture; 2]) -> Result<Self, String> {
        let mut result = Self {
            device: ptr::null_mut(),
            buffer: ptr::null_mut(),
            transfer: ptr::null_mut(),
            state: ptr::null_mut(),
            shader: ptr::null_mut(),
            sampler: ptr::null_mut(),
            bytes: 0,
        };
        // SAFETY: As the caller promises; every object made here is released on drop.
        unsafe {
            result.device = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_GPU_DEVICE_POINTER).cast();
            if result.device.is_null() {
                return Err("the renderer has no GPU device".into());
            }
            let code = include_bytes!("../packed444-planes.spv");
            result.shader = SDL_CreateGPUShader(result.device, &SDL_GPUShaderCreateInfo {
                code: code.as_ptr(),
                code_size: code.len(),
                entrypoint: c"fragment".as_ptr(),
                format: SDL_GPU_SHADERFORMAT_SPIRV,
                stage: SDL_GPU_SHADERSTAGE_FRAGMENT,
                num_samplers: 2,
                num_uniform_buffers: 1,
                ..Default::default()
            });
            if result.shader.is_null() {
                return Err(format!("the packed plane shader cannot be created: {}", crate::sdl::error()));
            }
            // The shader loads texels by position; the sampler only completes the binding.
            result.sampler = SDL_CreateGPUSampler(result.device, &SDL_GPUSamplerCreateInfo {
                min_filter: SDL_GPU_FILTER_NEAREST, mag_filter: SDL_GPU_FILTER_NEAREST, mipmap_mode: SDL_GPU_SAMPLERMIPMAPMODE_NEAREST,
                address_mode_u: SDL_GPU_SAMPLERADDRESSMODE_CLAMP_TO_EDGE, address_mode_v: SDL_GPU_SAMPLERADDRESSMODE_CLAMP_TO_EDGE,
                address_mode_w: SDL_GPU_SAMPLERADDRESSMODE_CLAMP_TO_EDGE, ..Default::default() });
            if result.sampler.is_null() {
                return Err(format!("the packed plane sampler cannot be created: {}", crate::sdl::error()));
            }
            let bindings = textures.map(|texture| SDL_GPUTextureSamplerBinding { texture, sampler: result.sampler });
            result.state = SDL_CreateGPURenderState(renderer, &SDL_GPURenderStateCreateInfo {
                fragment_shader: result.shader,
                num_sampler_bindings: 2,
                sampler_bindings: bindings.as_ptr(),
                ..Default::default()
            });
            if result.state.is_null() {
                return Err(format!("the packed plane render state cannot be created: {}", crate::sdl::error()));
            }
        }
        Ok(result)
    }

    /// Draws the planes bound in `for_planes`, once the picture is copied into them, into `target`.
    #[cfg(unix)]
    pub fn draw_planes(&self, renderer: *mut SDL_Renderer, target: *mut SDL_Texture, parameters: &[u32; 12]) -> bool {
        // SAFETY: The live renderer of the presenting thread and objects made on it.
        unsafe { !self.sampler.is_null() && self.fill(renderer, target, parameters) }
    }

    /// Runs the shader over `target` with `parameters`.
    unsafe fn fill(&self, renderer: *mut SDL_Renderer, target: *mut SDL_Texture, parameters: &[u32; 12]) -> bool {
        // SAFETY: As the callers promise: the renderer and target are live on this thread.
        unsafe {
            if !SDL_SetGPURenderStateFragmentUniforms(self.state, 0, parameters.as_ptr().cast(), 48) {
                return false;
            }
            let previous = SDL_GetRenderTarget(renderer);
            if !SDL_SetRenderTarget(renderer, target) {
                return false;
            }
            let ok = SDL_SetGPURenderState(renderer, self.state)
                && SDL_RenderFillRect(renderer, ptr::null())
                && SDL_FlushRenderer(renderer);
            SDL_SetGPURenderState(renderer, ptr::null_mut());
            SDL_SetRenderTarget(renderer, previous);
            ok
        }
    }

    #[cfg(windows)]
    pub fn upload(&mut self, frame: &media::Frame) -> bool {
        // SAFETY: The renderer owns native; FFmpeg keeps the decoder surface alive during the copy.
        frame.info().kind == media::FRAME_D3D11
            && self.native.as_ref().is_some_and(|native| native.upload(frame))
    }

    #[cfg(all(windows, test))]
    pub fn present(
        &mut self,
        renderer: *mut SDL_Renderer,
        parameters: &[u32; 12],
        destination: &SDL_FRect,
    ) -> bool {
        self.present_warped(renderer, parameters, destination, &super::frame_warp::WarpDraw::default())
    }

    /// Draws the picture into `destination` of the back buffer, shifted by Frame Warp's `warp`.
    #[cfg(windows)]
    pub fn present_warped(
        &mut self,
        renderer: *mut SDL_Renderer,
        parameters: &[u32; 12],
        destination: &SDL_FRect,
        warp: &super::frame_warp::WarpDraw,
    ) -> bool {
        // SAFETY: All renderer calls and the native draw run on the presenting thread.
        unsafe {
            let properties = SDL_GetRendererProperties(renderer);
            let swapchain = SDL_GetPointerProperty(
                properties,
                SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER,
                ptr::null_mut(),
            );
            let (mut width, mut height) = (0, 0);
            if swapchain.is_null() || !SDL_GetCurrentRenderOutputSize(renderer, &mut width, &mut height) {
                if let Some(native) = &self.native { native.release_back_buffer(); }
                return false;
            }
            // The kept view of the back buffer goes first if SDL is about to resize it at the flush.
            if let Some(native) = &self.native { native.keep_back_buffer_if(swapchain as usize, (width, height)); }
            if !SDL_FlushRenderer(renderer) {
                if let Some(native) = &self.native { native.release_back_buffer(); }
                return false;
            }
            let output = SDL_GetNumberProperty(
                properties,
                SDL_PROP_RENDERER_OUTPUT_COLORSPACE_NUMBER,
                i64::from(SDL_COLORSPACE_SRGB.0),
            );
            let mut scale = 1.0;
            if !SDL_GetRenderColorScale(renderer, &mut scale) {
                return false;
            }
            scale *= SDL_GetFloatProperty(properties, SDL_PROP_RENDERER_SDR_WHITE_POINT_FLOAT, 1.0);
            let destination = [destination.x, destination.y, destination.w, destination.h];
            self.native.as_ref().is_some_and(|native| native.present(
                swapchain,
                (width, height),
                parameters,
                &destination,
                output == i64::from(SDL_COLORSPACE_SRGB_LINEAR.0),
                scale,
                warp,
            ))
        }
    }

    pub fn draw(
        &mut self,
        renderer: *mut SDL_Renderer,
        target: *mut SDL_Texture,
        frame: &media::Frame,
        parameters: &[u32; 12],
    ) -> bool {
        #[cfg(target_os = "macos")]
        let mapped = (frame.info().kind == media::FRAME_VIDEOTOOLBOX).then(|| frame.map_to_memory());
        #[cfg(target_os = "macos")]
        let frame = match &mapped {
            Some(Some(frame)) => frame,
            Some(None) => return false,
            None => frame,
        };
        let info = frame.info();
        // SAFETY: The frame owns initialized raw planes. The validated coded dimensions
        // bound each row copy; SDL owns every mapped buffer and GPU object below.
        unsafe {
            // SDL may resize its buffers at the flush: the presenting draw's view is let go first.
            #[cfg(windows)]
            if let Some(native) = &self.native { native.release_back_buffer(); }
            if !SDL_FlushRenderer(renderer) {
                return false;
            }
            #[cfg(windows)]
            if let Some(native) = &self.native {
                return info.kind == media::FRAME_D3D11
                    && native.draw(frame, parameters);
            }
            if !matches!(info.kind, media::FRAME_NV12 | media::FRAME_P010) {
                return false;
            }
            let row = info.width as usize * if info.ten_bit != 0 { 2 } else { 1 };
            if info.linesize[0] < row as i32 || info.linesize[1] < row as i32 {
                return false;
            }
            let mapped = SDL_MapGPUTransferBuffer(self.device, self.transfer, true).cast::<u8>();
            if mapped.is_null() {
                return false;
            }
            let mut offset = 0;
            for plane in 0..2 {
                for y in 0..info.height as usize / if plane == 0 { 1 } else { 2 } {
                    ptr::copy_nonoverlapping(
                        info.data[plane].add(y * info.linesize[plane] as usize),
                        mapped.add(offset),
                        row,
                    );
                    offset += row;
                }
            }
            SDL_UnmapGPUTransferBuffer(self.device, self.transfer);
            let commands = SDL_AcquireGPUCommandBuffer(self.device);
            if commands.is_null() {
                return false;
            }
            let copy = SDL_BeginGPUCopyPass(commands);
            SDL_UploadToGPUBuffer(
                copy,
                &SDL_GPUTransferBufferLocation {
                    transfer_buffer: self.transfer,
                    offset: 0,
                },
                &SDL_GPUBufferRegion {
                    buffer: self.buffer,
                    offset: 0,
                    size: self.bytes,
                },
                true,
            );
            SDL_EndGPUCopyPass(copy);
            if !SDL_SubmitGPUCommandBuffer(commands) {
                return false;
            }
            self.fill(renderer, target, parameters)
        }
    }
}

impl Drop for Reconstruction {
    fn drop(&mut self) {
        // SAFETY: These resources are owned here; the renderer/device outlives this object.
        unsafe {
            if !self.state.is_null() {
                SDL_DestroyGPURenderState(self.state);
            }
            if !self.buffer.is_null() {
                SDL_ReleaseGPUBuffer(self.device, self.buffer);
            }
            if !self.transfer.is_null() {
                SDL_ReleaseGPUTransferBuffer(self.device, self.transfer);
            }
            if !self.shader.is_null() {
                SDL_ReleaseGPUShader(self.device, self.shader);
            }
            if !self.sampler.is_null() {
                SDL_ReleaseGPUSampler(self.device, self.sampler);
            }
        }
    }
}

#[cfg(test)]
mod tests;
