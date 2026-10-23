use std::ptr;
use std::time::Instant;
use sdl3_sys::everything::*;
use super::stage_times::Stages;
use super::upscale::Pass;

/// The presenter's stages of a PyroWave picture (`Reconstruction::draw_timed`).
pub const UPLOAD_AND_PASSES: [&str; 2] = ["upload", "passes"];
pub const UPLOAD: usize = 0;
pub const PASSES: usize = 1;

pub fn probe(renderer: *mut SDL_Renderer, ten_bit: bool) -> Result<(), String> {
    let data: &[u8] = if ten_bit { include_bytes!("../../tests/fixtures/pyrowave-420-10.bin") }
        else { include_bytes!("../../tests/fixtures/pyrowave-420-8.bin") };
    let mut decoder = super::pyrowave::Decoder::new(132, 98, false)?;
    decoder.coefficients(data)?;
    let frame = decoder.coefficient_frame()?;
    unsafe {
        let target = SDL_CreateTexture(renderer, SDL_PIXELFORMAT_RGBA64_FLOAT, SDL_TEXTUREACCESS_TARGET, 132, 98);
        if target.is_null() { return Err(crate::sdl::error()); }
        let gpu = Reconstruction::new(renderer, target, [132, 98, 160, 128, 0, 0, 5 | (u32::from(ten_bit) << 4), 1.0f32.to_bits()]);
        let mut ok = gpu.as_ref().is_some_and(|gpu| gpu.draw(renderer, target, &frame));
        let previous = SDL_GetRenderTarget(renderer);
        if ok && SDL_SetRenderTarget(renderer, target) {
            let readback = SDL_RenderReadPixels(renderer, ptr::null());
            ok = !readback.is_null();
            if ok { SDL_DestroySurface(readback); }
        } else { ok = false; }
        SDL_SetRenderTarget(renderer, previous);
        drop(gpu);
        SDL_DestroyTexture(target);
        if ok { Ok(()) } else { Err(format!("Wavelet shaders unavailable: {}", crate::sdl::error())) }
    }
}

pub struct Reconstruction {
    passes: Vec<Pass>,
    textures: [*mut SDL_Texture; 2],
    parameters: [u32; 8],
}

impl Reconstruction {
    pub fn new(renderer: *mut SDL_Renderer, target: *mut SDL_Texture, parameters: [u32; 8]) -> Option<Self> {
        let mut result = Self { passes: Vec::new(), textures: [ptr::null_mut(); 2], parameters };
        unsafe {
            for texture in &mut result.textures {
                *texture = SDL_CreateTexture(renderer, SDL_PIXELFORMAT_RGBA64_FLOAT, SDL_TEXTUREACCESS_TARGET, parameters[2] as i32, parameters[3] as i32);
                if texture.is_null() { return None; }
                SDL_SetTextureBlendMode(*texture, SDL_BLENDMODE_NONE);
                SDL_SetTextureScaleMode(*texture, SDL_SCALEMODE_LINEAR);
            }
            for (input, output) in [(result.textures[0], result.textures[1]), (result.textures[1], result.textures[0]), (result.textures[0], target)] {
                result.passes.push(Pass::new(renderer, input, output, include_bytes!("../pyrowave.hlsl"), include_bytes!("../pyrowave.spv"))?);
            }
        }
        Some(result)
    }

    pub fn draw(&self, renderer: *mut SDL_Renderer, target: *mut SDL_Texture, frame: &crate::media::Frame) -> bool {
        self.draw_timed(renderer, target, frame, &mut Stages::new(UPLOAD_AND_PASSES))
    }

    /// Draws the frame and adds the CPU time of the upload and of submitting the passes to
    /// `stages` (the GPU runs on after the submit returns).
    pub fn draw_timed(&self, renderer: *mut SDL_Renderer, target: *mut SDL_Texture, frame: &crate::media::Frame, stages: &mut Stages<2>) -> bool {
        let info = frame.info();
        let mut parameters = self.parameters;
        unsafe {
            // Preserve the untouched frequency bands in both targets; each pass writes
            // only its active rectangle instead of copying the whole picture ten times.
            let step = Instant::now();
            for texture in self.textures {
                if !SDL_UpdateTexture(texture, ptr::null(), info.data[0].cast(), info.linesize[0]) { return false; }
            }
            stages.add(UPLOAD, step);
            let step = Instant::now();
            for level in (0..5).rev() {
                parameters[4] = level;
                for pass in 0..2 {
                    parameters[5] = pass as u32;
                    if !self.passes[pass].draw(renderer, self.textures[pass], self.textures[1 - pass], &parameters,
                        Some((parameters[2] >> level, parameters[3] >> level))) { return false; }
                }
            }
            parameters[5] = 2;
            let ok = self.passes[2].draw(renderer, self.textures[0], target, &parameters, None);
            stages.add(PASSES, step);
            ok
        }
    }
}

impl Drop for Reconstruction {
    fn drop(&mut self) {
        self.passes.clear();
        for texture in self.textures { if !texture.is_null() { unsafe { SDL_DestroyTexture(texture) }; } }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "manual 1080p GPU latency measurement"]
    fn gpu_wavelet_1080p_latency() {
        unsafe {
            assert!(SDL_Init(SDL_INIT_VIDEO));
            #[cfg(windows)]
            {
                SDL_SetHint(SDL_HINT_RENDER_DRIVER, c"direct3d11".as_ptr());
                SDL_SetHint(SDL_HINT_RENDER_DIRECT3D_THREADSAFE, c"1".as_ptr());
            }
            let window = SDL_CreateWindow(c"Wavelet latency".as_ptr(), 1920, 1080, SDL_WINDOW_HIDDEN);
            let renderer = crate::sdl::create_renderer(window, true);
            assert!(!renderer.is_null());
            let target = SDL_CreateTexture(renderer, SDL_PIXELFORMAT_RGBA64_FLOAT, SDL_TEXTUREACCESS_TARGET, 1920, 1080);
            for full in [false, true] {
                let mut decoder = super::super::pyrowave::Decoder::new(1920, 1080, full).unwrap();
                let mut bytes = vec![1, 0, 0, 0, 8, 0, 0, 0];
                bytes.extend_from_slice(&(0x8000_0000u32 | 1919 | (1079 << 14)).to_le_bytes());
                bytes.extend_from_slice(&(u32::from(full) << 26).to_le_bytes());
                let gpu = Reconstruction::new(renderer, target, [1920, 1080, 1920, 1088, 0, 0, 5 | (u32::from(full) << 5), 1.0f32.to_bits()]).unwrap();
                let start = std::time::Instant::now();
                let mut stages = [0.0f64; 3];
                for _ in 0..30 {
                    let step = std::time::Instant::now();
                    decoder.coefficients(&bytes).unwrap();
                    let frame = decoder.coefficient_frame().unwrap();
                    stages[0] += step.elapsed().as_secs_f64();
                    let step = std::time::Instant::now();
                    assert!(gpu.draw(renderer, target, &frame));
                    stages[1] += step.elapsed().as_secs_f64();
                    let step = std::time::Instant::now();
                    assert!(SDL_SetRenderTarget(renderer, target));
                    let pixels = SDL_RenderReadPixels(renderer, &SDL_Rect { x: 0, y: 0, w: 1, h: 1 });
                    assert!(!pixels.is_null());
                    SDL_DestroySurface(pixels);
                    SDL_SetRenderTarget(renderer, ptr::null_mut());
                    stages[2] += step.elapsed().as_secs_f64();
                }
                eprintln!("1080p 444={full}: {:.2} ms/frame (empty entropy, allocation + upload + wavelets + synchronous one-pixel readback)", start.elapsed().as_secs_f64() * 1000.0 / 30.0);
                eprintln!("CPU coefficients / submit / sync readback ms: {:?}", stages.map(|seconds| seconds * 1000.0 / 30.0));
            }
            SDL_DestroyTexture(target); SDL_DestroyRenderer(renderer); SDL_DestroyWindow(window); SDL_Quit();
        }
    }

    #[test]
    #[ignore = "requires a D3D11 or Vulkan display"]
    fn gpu_wavelets_match_reference_planes() {
        unsafe {
            assert!(SDL_Init(SDL_INIT_VIDEO), "{}", crate::sdl::error());
            #[cfg(windows)]
            {
                SDL_SetHint(SDL_HINT_RENDER_DRIVER, c"direct3d11".as_ptr());
                SDL_SetHint(SDL_HINT_RENDER_DIRECT3D_THREADSAFE, c"1".as_ptr());
            }
            let window = SDL_CreateWindow(c"Wavelet conformance".as_ptr(), 132, 98, SDL_WINDOW_HIDDEN);
            let renderer = crate::sdl::create_renderer(window, true);
            assert!(!renderer.is_null(), "{}", crate::sdl::error());
            let cases: [(&[u8], bool, bool); 4] = [
                (include_bytes!("../../tests/fixtures/pyrowave-420-8.bin"), false, false),
                (include_bytes!("../../tests/fixtures/pyrowave-444-8.bin"), true, false),
                (include_bytes!("../../tests/fixtures/pyrowave-420-10.bin"), false, true),
                (include_bytes!("../../tests/fixtures/pyrowave-444-10.bin"), true, true),
            ];
            for (data, full, ten) in cases {
                let (width, height) = if full { (131, 97) } else { (132, 98) };
                let mut decoder = super::super::pyrowave::Decoder::new(width, height, full).unwrap();
                decoder.coefficients(data).unwrap();
                let frame = decoder.coefficient_frame().unwrap();
                decoder.decode(data).unwrap();
                let target = SDL_CreateTexture(renderer, SDL_PIXELFORMAT_RGBA64_FLOAT, SDL_TEXTUREACCESS_TARGET, width as i32, height as i32);
                let gpu = Reconstruction::new(renderer, target, [width as u32, height as u32, 160, 128, 0, 0, 5 | (u32::from(ten) << 4) | (u32::from(full) << 5), 1.0f32.to_bits()]).unwrap_or_else(|| panic!("wavelet shaders: {}", crate::sdl::error()));
                assert!(gpu.draw(renderer, target, &frame), "{}", crate::sdl::error());
                assert!(SDL_GetRenderTarget(renderer).is_null());
                assert!(SDL_SetRenderTarget(renderer, gpu.textures[0]));
                let readback = SDL_RenderReadPixels(renderer, ptr::null());
                assert!(!readback.is_null(), "{}", crate::sdl::error());
                let pixels = SDL_ConvertSurface(readback, SDL_PIXELFORMAT_RGBA128_FLOAT);
                SDL_DestroySurface(readback);
                assert!(!pixels.is_null());
                let mut maximum_error = 0.0f32;
                let mut squared_error = 0.0f64;
                let mut samples = 0;
                for plane in 0..3 {
                    let shift = usize::from(plane != 0 && !full);
                    for y in 0..height >> shift {
                        let row = std::slice::from_raw_parts(((*pixels).pixels as *const u8).add(y * (*pixels).pitch as usize).cast::<f32>(), 160 * 4);
                        for x in 0..width >> shift {
                            let expected = decoder.sample(plane, x << shift, y << shift);
                            let actual = (row[x * 4 + plane] + 0.5).clamp(0.0, 1.0);
                            maximum_error = maximum_error.max((actual - expected).abs());
                            squared_error += f64::from(actual - expected).powi(2);
                            samples += 1;
                        }
                    }
                }
                let rms = (squared_error / f64::from(samples)).sqrt();
                eprintln!("444={full}, ten={ten}: max={maximum_error}, rms={rms}");
                assert!(maximum_error < 1.0 / 255.0 && rms < 0.001, "GPU wavelet error exceeds one 8-bit code value");
                SDL_DestroySurface(pixels);
                SDL_SetRenderTarget(renderer, ptr::null_mut());
                drop(gpu);
                SDL_DestroyTexture(target);
            }
            SDL_DestroyRenderer(renderer); SDL_DestroyWindow(window); SDL_Quit();
        }
    }
}
