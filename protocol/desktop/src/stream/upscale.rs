use std::ptr;

use sdl3_sys::everything::*;

use crate::config::Upscaling;

pub(super) struct Pass {
    #[cfg(windows)]
    native: Option<super::d3d11::Filter>,
    device: *mut SDL_GPUDevice,
    shader: *mut SDL_GPUShader,
    state: *mut SDL_GPURenderState,
}

impl Pass {
    pub(super) unsafe fn new(renderer: *mut SDL_Renderer, _input: *mut SDL_Texture, _output: *mut SDL_Texture, _hlsl: &[u8], spirv: &[u8]) -> Option<Self> {
        let mut pass = Self {
            #[cfg(windows)]
            native: None,
            device: ptr::null_mut(), shader: ptr::null_mut(), state: ptr::null_mut(),
        };
        #[cfg(windows)]
        {
            let input = SDL_GetPointerProperty(SDL_GetTextureProperties(_input), SDL_PROP_TEXTURE_D3D11_TEXTURE_POINTER, ptr::null_mut());
            let output = SDL_GetPointerProperty(SDL_GetTextureProperties(_output), SDL_PROP_TEXTURE_D3D11_TEXTURE_POINTER, ptr::null_mut());
            if !input.is_null() && !output.is_null() {
                pass.native = super::d3d11::Filter::new(input, output, _hlsl).map_err(|e| eprintln!("nightjar: GPU filter: {e}")).ok();
                return pass.native.is_some().then_some(pass);
            }
        }
        pass.device = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_GPU_DEVICE_POINTER).cast();
        if pass.device.is_null() { return None; }
        let code = spirv;
        pass.shader = SDL_CreateGPUShader(pass.device, &SDL_GPUShaderCreateInfo { code: code.as_ptr(), code_size: code.len(),
            entrypoint: c"fragment".as_ptr(), format: SDL_GPU_SHADERFORMAT_SPIRV, stage: SDL_GPU_SHADERSTAGE_FRAGMENT,
            num_samplers: 1, num_uniform_buffers: 1, ..Default::default() });
        if pass.shader.is_null() { return None; }
        pass.state = SDL_CreateGPURenderState(renderer, &SDL_GPURenderStateCreateInfo { fragment_shader: pass.shader, ..Default::default() });
        (!pass.state.is_null()).then_some(pass)
    }

    pub(super) unsafe fn draw(&self, renderer: *mut SDL_Renderer, input: *mut SDL_Texture, output: *mut SDL_Texture, parameters: &[u32; 8], area: Option<(u32, u32)>) -> bool {
        if !SDL_FlushRenderer(renderer) { return false; }
        #[cfg(windows)]
        if let Some(native) = &self.native { return native.draw(parameters, area); }
        let previous = SDL_GetRenderTarget(renderer);
        if !SDL_SetRenderTarget(renderer, output) { return false; }
        let area = area.map(|(width, height)| SDL_FRect { x: 0.0, y: 0.0, w: width as f32, h: height as f32 });
        let area = area.as_ref().map_or(ptr::null(), |area| area as *const _);
        let ok = SDL_SetGPURenderStateFragmentUniforms(self.state, 0, parameters.as_ptr().cast(), 32)
            && SDL_SetGPURenderState(renderer, self.state)
            && SDL_RenderTexture(renderer, input, ptr::null(), area) && SDL_FlushRenderer(renderer);
        SDL_SetGPURenderState(renderer, ptr::null_mut());
        SDL_SetRenderTarget(renderer, previous);
        ok
    }
}

impl Drop for Pass {
    fn drop(&mut self) {
        unsafe {
            if !self.state.is_null() { SDL_DestroyGPURenderState(self.state); }
            if !self.shader.is_null() { SDL_ReleaseGPUShader(self.device, self.shader); }
        }
    }
}

pub struct Upscaler {
    passes: Vec<Pass>,
    textures: [*mut SDL_Texture; 3],
    input: (i32, i32),
    output: (i32, i32),
    mode: Upscaling,
}

impl Upscaler {
    pub fn new(renderer: *mut SDL_Renderer, input: (i32, i32), output: (i32, i32), mode: Upscaling) -> Option<Self> {
        let mut result = Self { passes: Vec::new(), textures: [ptr::null_mut(); 3], input, output, mode };
        unsafe {
            for (i, size) in [input, output, output].into_iter().enumerate() {
                let texture = SDL_CreateTexture(renderer, SDL_PIXELFORMAT_RGBA32, SDL_TEXTUREACCESS_TARGET, size.0, size.1);
                if texture.is_null() { return None; }
                result.textures[i] = texture;
                SDL_SetTextureBlendMode(texture, SDL_BLENDMODE_NONE);
                SDL_SetTextureScaleMode(texture, SDL_SCALEMODE_LINEAR);
            }
            result.passes.push(Pass::new(renderer, result.textures[0], result.textures[1], include_bytes!("../upscale.hlsl"), include_bytes!("../upscale.spv"))?);
            if mode == Upscaling::Fsr1 { result.passes.push(Pass::new(renderer, result.textures[1], result.textures[2], include_bytes!("../upscale.hlsl"), include_bytes!("../upscale.spv"))?); }
        }
        Some(result)
    }

    pub fn matches(&self, input: (i32, i32), output: (i32, i32), mode: Upscaling) -> bool {
        self.input == input && self.output == output && self.mode == mode
    }

    pub fn draw(&self, renderer: *mut SDL_Renderer, source: *mut SDL_Texture, sharpness: u32) -> Option<*mut SDL_Texture> {
        unsafe {
            let previous = SDL_GetRenderTarget(renderer);
            if !SDL_SetRenderTarget(renderer, self.textures[0]) { return None; }
            let ok = SDL_RenderTexture(renderer, source, ptr::null(), ptr::null()) && SDL_FlushRenderer(renderer);
            SDL_SetRenderTarget(renderer, previous);
            if !ok { return None; }
            for (i, pass) in self.passes.iter().enumerate() {
                let input = if i == 0 { self.input } else { self.output };
                let parameters = [
                    (input.0 as f32).to_bits(), (input.1 as f32).to_bits(),
                    (self.output.0 as f32).to_bits(), (self.output.1 as f32).to_bits(),
                    (1.0 + sharpness.min(100) as f32 / 100.0).to_bits(),
                    2.0f32.powf(-2.0 * (1.0 - sharpness.min(100) as f32 / 100.0)).to_bits(),
                    if self.mode == Upscaling::Sgsr1 { 3 } else if i == 0 { 1 } else { 2 }, 0,
                ];
                if !pass.draw(renderer, self.textures[i], self.textures[i + 1], &parameters, None) { return None; }
            }
            Some(self.textures[self.passes.len()])
        }
    }
}

impl Drop for Upscaler {
    fn drop(&mut self) {
        self.passes.clear();
        for texture in self.textures {
            if !texture.is_null() { unsafe { SDL_DestroyTexture(texture) }; }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a D3D11 or Vulkan display"]
    fn gpu_scalers_preserve_flat_colors_and_release_targets() {
        unsafe {
            assert!(SDL_Init(SDL_INIT_VIDEO), "{}", crate::sdl::error());
            #[cfg(windows)]
            {
                SDL_SetHint(SDL_HINT_RENDER_DRIVER, c"direct3d11".as_ptr());
                SDL_SetHint(SDL_HINT_RENDER_DIRECT3D_THREADSAFE, c"1".as_ptr());
            }
            let window = SDL_CreateWindow(c"Upscaling test".as_ptr(), 128, 128, SDL_WINDOW_HIDDEN);
            let renderer = crate::sdl::create_renderer(window, true);
            assert!(!renderer.is_null(), "{}", crate::sdl::error());
            let input = SDL_CreateTexture(renderer, SDL_PIXELFORMAT_RGBA32, SDL_TEXTUREACCESS_STATIC, 32, 32);
            assert!(!input.is_null(), "{}", crate::sdl::error());
            for color in [Some(0u8), Some(64), Some(128), Some(255), None] {
                let quadrant = |x, y| match (x, y) { (false, false) => [255, 0, 0, 255],
                    (true, false) => [0, 255, 0, 255], (false, true) => [0, 0, 255, 255], (true, true) => [255; 4] };
                let pixels: Vec<_> = (0..32 * 32).flat_map(|i| color.map_or_else(|| quadrant(i % 32 >= 16, i / 32 >= 16), |c| [c, c, c, 255])).collect();
                assert!(SDL_UpdateTexture(input, ptr::null(), pixels.as_ptr().cast(), 32 * 4));
                for mode in [Upscaling::Fsr1, Upscaling::Sgsr1] {
                    let upscaler = Upscaler::new(renderer, (32, 32), (128, 128), mode).expect("GPU scaler");
                    let output = upscaler.draw(renderer, input, 50).expect("GPU draw");
                    assert!(SDL_GetRenderTarget(renderer).is_null());
                    assert!(SDL_SetRenderTarget(renderer, output));
                    let surface = SDL_RenderReadPixels(renderer, ptr::null());
                    assert!(!surface.is_null(), "{}", crate::sdl::error());
                    let rgba = SDL_ConvertSurface(surface, SDL_PIXELFORMAT_RGBA32);
                    SDL_DestroySurface(surface);
                    assert!(!rgba.is_null());
                    for y in 0..128 {
                        let row = std::slice::from_raw_parts(((*rgba).pixels as *const u8).add(y * (*rgba).pitch as usize), 128 * 4);
                        for (x, pixel) in row.chunks_exact(4).enumerate() {
                            if color.is_none() && (x.abs_diff(64) < 8 || y.abs_diff(64) < 8) { continue; }
                            let expected = color.map_or_else(|| quadrant(x >= 64, y >= 64), |c| [c, c, c, 255]);
                            assert!(pixel[..3].iter().zip(&expected).all(|(c, e)| c.abs_diff(*e) <= 2), "{mode:?} at ({x},{y}): {expected:?} -> {pixel:?}");
                        }
                    }
                    SDL_DestroySurface(rgba);
                    SDL_SetRenderTarget(renderer, ptr::null_mut());
                }
            }
            SDL_DestroyTexture(input);
            SDL_DestroyRenderer(renderer);
            SDL_DestroyWindow(window);
            SDL_Quit();
        }
    }
}
