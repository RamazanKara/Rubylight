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
    let mut decoder = media::VideoDecoder::open(codec, hardware, 1, true)?;
    let status = decoder.send(data, 0);
    if status < 0 {
        return Err(format!("decoder rejected the sampling fixture ({status})"));
    }
    let frame = match decoder.receive() {
        media::Received::Frame(frame) => frame,
        media::Received::None => return Err("decoder produced no fixture frame".into()),
        media::Received::Error => return Err("fixture decoding failed".into()),
    };
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
        let mut reconstruction = Reconstruction::new(renderer, target, &parameters);
        let mut valid = reconstruction
            .as_mut()
            .is_some_and(|r| r.draw(renderer, target, &frame, &parameters));
        let mut reason = format!("GPU reconstruction unavailable: {}", crate::sdl::error());
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

fn fixture_rgb(x: usize, y: usize, ten_bit: bool, packing: u32) -> [f32; 3] {
    let maximum = if ten_bit { 1023 } else { 255 };
    let luma = ((x + y * 13) & maximum) as f32;
    let (u, v) = if packing == 2 {
        let base = (x / 2 * 7 + y / 2 * 19 + 17) & maximum;
        let mut uv = [base, maximum - base];
        let detail = if x % 2 != 0 {
            Some([(x / 2 + (64 + y) * 13) & maximum, (64 + x / 2 + (64 + y) * 13) & maximum])
        } else if y % 2 != 0 {
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
    } else if x % 2 == 0 {
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
            let code = include_bytes!("../packed444.spv");
            result.shader = SDL_CreateGPUShader(
                result.device,
                &SDL_GPUShaderCreateInfo {
                    code: code.as_ptr(),
                    code_size: code.len(),
                    entrypoint: c"fragment".as_ptr(),
                    format: SDL_GPU_SHADERFORMAT_SPIRV,
                    stage: SDL_GPU_SHADERSTAGE_FRAGMENT,
                    num_storage_buffers: 1,
                    num_uniform_buffers: 1,
                    ..Default::default()
                },
            );
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

    #[cfg(windows)]
    pub fn upload(&mut self, frame: &media::Frame) -> bool {
        // SAFETY: The renderer owns native; FFmpeg keeps the decoder surface alive during the copy.
        frame.info().kind == media::FRAME_D3D11
            && self.native.as_ref().is_some_and(|native| native.upload(frame))
    }

    #[cfg(windows)]
    pub fn present(
        &mut self,
        renderer: *mut SDL_Renderer,
        parameters: &[u32; 12],
        destination: &SDL_FRect,
    ) -> bool {
        // SAFETY: All renderer calls and the native draw run on the presenting thread.
        unsafe {
            if !SDL_FlushRenderer(renderer) {
                return false;
            }
            let properties = SDL_GetRendererProperties(renderer);
            let swapchain = SDL_GetPointerProperty(
                properties,
                SDL_PROP_RENDERER_D3D11_SWAPCHAIN_POINTER,
                ptr::null_mut(),
            );
            if swapchain.is_null() {
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
                parameters,
                &destination,
                output == i64::from(SDL_COLORSPACE_SRGB_LINEAR.0),
                scale,
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
        let info = frame.info();
        // SAFETY: The frame owns initialized raw planes. The validated coded dimensions
        // bound each row copy; SDL owns every mapped buffer and GPU object below.
        unsafe {
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
            if !SDL_SetGPURenderStateFragmentUniforms(self.state, 0, parameters.as_ptr().cast(), 48)
            {
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    unsafe fn pixels(renderer: *mut SDL_Renderer) -> Vec<f32> {
        // SAFETY: Readback owns the temporary surfaces and copies their initialized rows.
        unsafe {
            let surface = SDL_RenderReadPixels(renderer, ptr::null());
            assert!(!surface.is_null(), "{}", crate::sdl::error());
            let converted = SDL_ConvertSurface(surface, SDL_PIXELFORMAT_RGBA128_FLOAT);
            SDL_DestroySurface(surface);
            assert!(!converted.is_null(), "{}", crate::sdl::error());
            let mut result = Vec::new();
            for y in 0..(*converted).h as usize {
                result.extend_from_slice(std::slice::from_raw_parts(
                    ((*converted).pixels as *const u8)
                        .add(y * (*converted).pitch as usize)
                        .cast::<f32>(),
                    (*converted).w as usize * 4,
                ));
            }
            SDL_DestroySurface(converted);
            result
        }
    }

    #[cfg(windows)]
    unsafe fn overlay(renderer: *mut SDL_Renderer) {
        // SAFETY: The live renderer belongs to this test's thread.
        unsafe {
            assert!(SDL_SetRenderDrawColor(renderer, 213, 55, 17, 255));
            assert!(SDL_RenderFillRect(
                renderer,
                &SDL_FRect {
                    x: 3.0,
                    y: 4.0,
                    w: 12.0,
                    h: 9.0
                }
            ));
        }
    }

    #[cfg(windows)]
    unsafe fn direct_matches_reconstruction(window: *mut SDL_Window, renderer: *mut SDL_Renderer) {
        use super::super::pipeline::Decoded;
        use super::super::present::{letterbox, VideoTexture};
        // SAFETY: The test owns the window, renderer, textures and decoder on this thread.
        unsafe {
            let hardware = media::Hardware::D3d11(crate::sdl::renderer_pointer(
                renderer,
                SDL_PROP_RENDERER_D3D11_DEVICE_POINTER,
            ));
            let target = SDL_CreateTexture(
                renderer,
                SDL_PIXELFORMAT_RGBA64_FLOAT,
                SDL_TEXTUREACCESS_TARGET,
                128,
                64,
            );
            assert!(!target.is_null());
            assert!(SDL_SetTextureScaleMode(target, SDL_SCALEMODE_LINEAR));
            let linear_output = SDL_GetNumberProperty(SDL_GetRendererProperties(renderer), SDL_PROP_RENDERER_OUTPUT_COLORSPACE_NUMBER, 0)
                == i64::from(SDL_COLORSPACE_SRGB_LINEAR.0);
            for (codec, format, data) in [
                (
                    media::Codec::Hevc,
                    nightjar_core::sdp::VIDEO_FORMAT_H265,
                    &include_bytes!("../../../../app/src/main/assets/packed444/hevc-8.bin")[..],
                ),
                (
                    media::Codec::Av1,
                    nightjar_core::sdp::VIDEO_FORMAT_AV1_MAIN8,
                    &include_bytes!("../../../../app/src/main/assets/packed444/av1-8.bin")[..],
                ),
                (
                    media::Codec::Hevc,
                    nightjar_core::sdp::VIDEO_FORMAT_H265_MAIN10,
                    &include_bytes!("../../../../app/src/main/assets/packed444/hevc-10.bin")[..],
                ),
                (
                    media::Codec::Av1,
                    nightjar_core::sdp::VIDEO_FORMAT_AV1_MAIN10,
                    &include_bytes!("../../../../app/src/main/assets/packed444/av1-10.bin")[..],
                ),
            ] {
                let mut decoder = media::VideoDecoder::open(codec, hardware, 1, true).unwrap();
                assert_eq!(decoder.send(data, 0), 0);
                let media::Received::Frame(frame) = decoder.receive() else {
                    panic!("no hardware fixture frame")
                };
                let mut decoded = Decoded {
                    frame,
                    setup: nightjar_core::session::VideoSetup {
                        format,
                        width: 128,
                        height: 128,
                        fps: 60,
                        chroma_packing: 1,
                        logical_width: 128,
                        logical_height: 64,
                        color_space: 1,
                        color_range: 1,
                    },
                    rtp: 0,
                    receive_us: 0,
                    enqueue_us: 0,
                    ready_us: 0,
                    host_latency: 0,
                };
                let ten_bit = format & nightjar_core::sdp::VIDEO_FORMAT_MASK_10BIT != 0;
                let mut parameters = [128, 64, 128, 64, u32::from(ten_bit), 1, 1, 0, 1.0f32.to_bits(), 0, 0, 0];
                let mut reference = Reconstruction::new(renderer, target, &parameters).unwrap();
                let mut video = VideoTexture::default();
                video.set_hdr_mode(false, None);
                for (range, packing, hdr) in (0..=1).flat_map(|range| (1..=2).flat_map(move |packing| [false, true].into_iter().filter(move |&hdr| !hdr || (ten_bit && linear_output)).map(move |hdr| (range, packing, hdr)))) {
                    decoded.setup.chroma_packing = packing;
                    parameters[7] = u32::from(hdr);
                    parameters[9] = u32::from(packing);
                    let mut direct_hdr = hdr.then(|| Reconstruction::new(renderer, ptr::null_mut(), &parameters).unwrap());
                    if let Some(direct) = &mut direct_hdr { assert!(direct.upload(&decoded.frame)); }
                    for matrix in 0..=2 {
                        decoded.setup.color_range = range;
                        decoded.setup.color_space = matrix;
                        parameters[5] = range as u32;
                        parameters[6] = matrix as u32;
                        assert!(reference.draw(renderer, target, &decoded.frame, &parameters));
                        if !hdr {
                            assert!(video.upload(renderer, &decoded));
                            assert!(video.texture.is_null(), "direct presentation allocated an intermediate RGB texture");
                        }
                        for (width, height) in [(128, 64), (240, 160), (99, 59)] {
                            assert!(SDL_SetWindowSize(window, width, height));
                            SDL_PumpEvents();
                            let (mut w, mut h) = (0, 0);
                            assert!(SDL_GetCurrentRenderOutputSize(renderer, &mut w, &mut h));
                            assert_eq!((w, h), (width, height));
                            let destination = letterbox((128, 64), (w, h));
                            for scale in [1.0, 0.6] {
                                assert!(SDL_SetRenderColorScale(renderer, scale));
                                assert!(SDL_SetRenderDrawColor(renderer, 0, 0, 0, 255));
                                assert!(SDL_RenderClear(renderer));
                                assert!(SDL_RenderTexture(
                                    renderer,
                                    target,
                                    ptr::null(),
                                    &destination
                                ));
                                overlay(renderer);
                                let expected = pixels(renderer);
                                // The reference stores FP16 and uses fixed-point hardware
                                // filtering. Bound HDR interpolation error by its scene range.
                                let hdr_range = if hdr {
                                    expected.iter().fold(1.0f32, |peak, value| peak.max(value.abs()))
                                } else { 1.0 };
                                assert!(SDL_RenderPresent(renderer));
                                // Redraws after Present and resize must retain the uploaded picture.
                                for _ in 0..2 {
                                    assert!(SDL_SetRenderDrawColor(renderer, 0, 0, 0, 255));
                                    assert!(SDL_RenderClear(renderer));
                                    assert!(
                                        if let Some(direct) = &mut direct_hdr { direct.present(renderer, &parameters, &destination) } else { video.draw(renderer, (w, h)) },
                                        "{}",
                                        crate::sdl::error()
                                    );
                                    overlay(renderer);
                                    let actual = pixels(renderer);
                                    assert_eq!(actual.len(), expected.len());
                                    let mut maximum = 0.0f32;
                                    let mut sum = 0.0;
                                    let mut worst = (0.0, 0.0);
                                    for (&a, &b) in expected.iter().zip(&actual) {
                                        assert!(b.is_finite());
                                        let error = (a - b).abs() / hdr_range;
                                        if error > maximum { worst = (a, b); }
                                        maximum = maximum.max(error);
                                        sum += error;
                                    }
                                    let mean = sum / actual.len() as f32;
                                    let tolerance = if hdr && destination.w == 128.0 && destination.h == 64.0 { 0.001 } else { 0.01 };
                                    assert!(maximum < tolerance && mean < 0.0008,
                                        "{codec:?} ten_bit={ten_bit} packing={packing} hdr={hdr} range={range} matrix={matrix} {w}x{h} scale={scale}: max={maximum}, mean={mean}, worst={worst:?}");
                                    assert!(SDL_RenderPresent(renderer));
                                }
                            }
                        }
                    }
                }
                video.destroy();
                if ten_bit && linear_output {
                    // Emulate display limits to exercise path selection without changing Windows HDR.
                    let properties = SDL_GetRendererProperties(renderer);
                    let enabled = SDL_GetBooleanProperty(properties, SDL_PROP_RENDERER_HDR_ENABLED_BOOLEAN, false);
                    let white = SDL_GetFloatProperty(properties, SDL_PROP_RENDERER_SDR_WHITE_POINT_FLOAT, 1.0);
                    let headroom = SDL_GetFloatProperty(properties, SDL_PROP_RENDERER_HDR_HEADROOM_FLOAT, 1.0);
                    assert!(SDL_SetBooleanProperty(properties, SDL_PROP_RENDERER_HDR_ENABLED_BOOLEAN, true));
                    assert!(SDL_SetFloatProperty(properties, SDL_PROP_RENDERER_SDR_WHITE_POINT_FLOAT, 2.0));
                    for direct in [true, false] {
                        let mut video = VideoTexture::new(direct);
                        for (peak, available) in [(320, 16.0), (4000, 16.0), (320, 16.0), (320, 1.0), (320, 16.0)] {
                            assert!(SDL_SetFloatProperty(properties, SDL_PROP_RENDERER_HDR_HEADROOM_FLOAT, available));
                            video.set_hdr_mode(true, Some(nightjar_core::control::HdrMetadata {
                                max_content_light_level: peak,
                                ..Default::default()
                            }));
                            assert!(video.upload(renderer, &decoded));
                            assert_eq!(video.texture.is_null(), direct && peak == 320 && available == 16.0);
                            assert!(video.draw(renderer, (99, 59)));
                            assert!(SDL_RenderPresent(renderer));
                        }
                        video.destroy();
                    }
                    SDL_SetBooleanProperty(properties, SDL_PROP_RENDERER_HDR_ENABLED_BOOLEAN, enabled);
                    SDL_SetFloatProperty(properties, SDL_PROP_RENDERER_SDR_WHITE_POINT_FLOAT, white);
                    SDL_SetFloatProperty(properties, SDL_PROP_RENDERER_HDR_HEADROOM_FLOAT, headroom);
                }
                drop(reference);
            }
            SDL_DestroyTexture(target);
        }
    }

    #[test]
    #[ignore = "requires an HEVC/AV1 hardware decoder and a D3D11 or Vulkan display"]
    fn packed_decode_and_gpu_reconstruction() {
        // SAFETY: This test owns SDL and all its resources on one thread.
        unsafe {
            assert!(SDL_Init(SDL_INIT_VIDEO), "{}", crate::sdl::error());
            #[cfg(windows)]
            {
                SDL_SetHint(SDL_HINT_RENDER_DRIVER, c"direct3d11".as_ptr());
                SDL_SetHint(SDL_HINT_RENDER_DIRECT3D_THREADSAFE, c"1".as_ptr());
            }
            let window =
                SDL_CreateWindow(c"Packed chroma test".as_ptr(), 128, 64, SDL_WINDOW_HIDDEN);
            let renderer = crate::sdl::create_renderer(window, true);
            assert!(!renderer.is_null(), "{}", crate::sdl::error());
            #[cfg(windows)]
            let hardware = media::Hardware::D3d11(crate::sdl::renderer_pointer(
                renderer,
                SDL_PROP_RENDERER_D3D11_DEVICE_POINTER,
            ));
            #[cfg(not(windows))]
            let hardware = media::Hardware::Vaapi;
            let modes = probe(renderer, hardware);
            #[cfg(windows)]
            direct_matches_reconstruction(window, renderer);
            SDL_DestroyRenderer(renderer);
            #[cfg(windows)]
            {
                let renderer = crate::sdl::create_renderer(window, false);
                assert!(!renderer.is_null(), "{}", crate::sdl::error());
                direct_matches_reconstruction(window, renderer);
                SDL_DestroyRenderer(renderer);
            }
            SDL_DestroyWindow(window);
            SDL_Quit();
            assert_eq!(
                modes.unwrap(),
                15,
                "8/10-bit HEVC/AV1 reconstruction must match CPU color conversion"
            );
        }
    }
}
