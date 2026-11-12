//! The stream's overlay, toasts and menu on an HDR10 swapchain image (`ui_pq.hlsl`).
//!
//! On a PQ output a frame that draws only the picture takes its swapchain image straight away and
//! the plane pass writes PQ into it (`present::VideoOnly`). SDL's own draws write linear light, so
//! the patched SDL drops them in such a frame, and a frame with the statistics overlay, a toast or
//! the stream menu used to go through SDL's RGBA16F back buffer and its full-screen PQ pass
//! instead: at 4K two more full-screen passes over 16-bit float for a few lines of text, about
//! 2.5 ms of an Iris Plus 655's render engine a picture (rust/PERFORMANCE.md, November 12; a Radeon
//! 760M showed 84 of 120 pictures with the overlay on, nightjar issue 104). Here
//! egui's meshes get a render state of Nightjar's own, which SDL draws in a video-only frame: its
//! fragment stage does SDL's textured draw and then the present's encode, so those frames stay
//! one pass too.

use sdl3_sys::everything::*;

const FRAGMENT_SPIRV: &[u8] = include_bytes!("../ui_pq_fragment.spv");

/// The fragment stage's constants (`Look` in ui_pq.hlsl).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Look {
    /// SDL's colour scale for a draw: the renderer's colour scale times the output's SDR white.
    pub color_scale: f32,
    /// The present's encoding, as `sdl::present_encoding` gives it: the mode (0 PQ, 1 gamma 2.2),
    /// the minimum in nits and 1 / (peak - minimum).
    pub mode: f32,
    pub min_nits: f32,
    pub inv_range: f32,
}

/// The constants for a renderer whose output has `sdr_white` (SDL's SDR white point, in units of
/// 80 nits) and `color_scale`, presenting with `encoding`.
pub fn look(sdr_white: f32, color_scale: f32, encoding: [f32; 4]) -> Look {
    let [mode, min_nits, inv_range, _] = encoding;
    // As `planes_gpu::direct_uniforms` reads it: gamma 2.2 needs a range.
    let gamma22 = mode == 1.0 && inv_range > 0.0;
    Look {
        color_scale: sdr_white * color_scale,
        mode: if gamma22 { 1.0 } else { 0.0 },
        min_nits: if gamma22 { min_nits } else { 0.0 },
        inv_range: if gamma22 { inv_range } else { 0.0 },
    }
}

/// The render state egui's meshes are drawn with in a video-only frame.
pub struct UiPq {
    renderer: *mut SDL_Renderer,
    device: *mut SDL_GPUDevice,
    shader: *mut SDL_GPUShader,
    state: *mut SDL_GPURenderState,
    look: Option<Look>,
}

impl UiPq {
    /// # Safety
    /// `renderer` must be the live SDL "gpu" renderer of the calling thread, and outlive this.
    pub unsafe fn new(renderer: *mut SDL_Renderer) -> Result<Self, String> {
        let device: *mut SDL_GPUDevice = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_GPU_DEVICE_POINTER).cast();
        if device.is_null() {
            return Err("the renderer has no GPU device".into());
        }
        let shader = SDL_CreateGPUShader(device, &SDL_GPUShaderCreateInfo {
            code: FRAGMENT_SPIRV.as_ptr(),
            code_size: FRAGMENT_SPIRV.len(),
            entrypoint: c"fragment".as_ptr(),
            format: SDL_GPU_SHADERFORMAT_SPIRV,
            stage: SDL_GPU_SHADERSTAGE_FRAGMENT,
            // The mesh's own texture, which SDL binds in the first slot of a textured draw.
            num_samplers: 1,
            num_uniform_buffers: 1,
            ..Default::default()
        });
        if shader.is_null() {
            return Err(format!("the overlay's PQ shader cannot be created: {}", crate::sdl::error()));
        }
        let state = SDL_CreateGPURenderState(renderer, &SDL_GPURenderStateCreateInfo { fragment_shader: shader, ..Default::default() });
        if state.is_null() {
            let error = format!("the overlay's PQ render state cannot be created: {}", crate::sdl::error());
            SDL_ReleaseGPUShader(device, shader);
            return Err(error);
        }
        Ok(Self { renderer, device, shader, state, look: None })
    }

    /// The state for this frame's meshes, with the renderer's constants as they are now; null if
    /// they cannot be set (the meshes would be drawn with stale ones).
    ///
    /// # Safety
    /// On the thread of the renderer this was made for, which is `renderer` and live.
    pub unsafe fn state(&mut self, renderer: *mut SDL_Renderer) -> *mut SDL_GPURenderState {
        if renderer != self.renderer {
            return std::ptr::null_mut();
        }
        let mut color_scale = 1.0;
        SDL_GetRenderColorScale(renderer, &mut color_scale);
        let sdr_white = SDL_GetFloatProperty(SDL_GetRendererProperties(renderer), SDL_PROP_RENDERER_SDR_WHITE_POINT_FLOAT, 1.0);
        let now = look(sdr_white, color_scale, crate::sdl::present_encoding(renderer));
        if self.look != Some(now) {
            // SDL flushes what was queued with the old constants before it takes the new ones.
            if !SDL_SetGPURenderStateFragmentUniforms(self.state, 0, (&now as *const Look).cast(), std::mem::size_of::<Look>() as u32) {
                self.look = None;
                return std::ptr::null_mut();
            }
            self.look = Some(now);
        }
        self.state
    }
}

impl Drop for UiPq {
    fn drop(&mut self) {
        // SAFETY: made in `new` on a renderer that is still live (`VideoTexture::destroy` runs
        // before the renderer goes); SDL flushes the draws queued with the state first.
        unsafe {
            SDL_DestroyGPURenderState(self.state);
            SDL_ReleaseGPUShader(self.device, self.shader);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_constants_are_sdls_colour_scale_and_the_presents_encoding() {
        // PQ: SDL's colour scale for a draw is the renderer's times the SDR white point.
        assert_eq!(look(2.5375, 1.0, [0.0; 4]), Look { color_scale: 2.5375, mode: 0.0, min_nits: 0.0, inv_range: 0.0 });
        assert_eq!(look(2.5, 0.5, [0.0; 4]).color_scale, 1.25);
        // Gamma 2.2 with its range, as the scanout present gets it.
        let encoding = crate::stream::wl_scanout::Encoding::constants(1, 0.05, 1000.05);
        assert_eq!(look(1.0, 1.0, encoding), Look { color_scale: 1.0, mode: 1.0, min_nits: 0.05, inv_range: 1.0 / 1000.0 });
        // An empty range is PQ, as for the plane pass.
        assert_eq!(look(1.0, 1.0, [1.0, 0.05, 0.0, 0.0]), look(1.0, 1.0, [0.0; 4]));
    }

    #[test]
    fn the_constants_fill_the_shaders_block() {
        assert_eq!(std::mem::size_of::<Look>(), 16);
        assert_eq!(std::mem::offset_of!(Look, inv_range), 12);
    }

    /// `ui_pq_fragment.spv` (glslang 16.3.0) is the shader as it is: a shader change without the
    /// module regenerated fails here, as does a module replaced without its stamp.
    #[test]
    fn the_module_was_built_from_the_shader_as_it_is() {
        use sha2::Digest;
        let sha256 = |bytes: &[u8]| -> String { sha2::Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect() };
        let stamp = include_str!("../ui_pq_fragment.spv.sha256");
        let recorded = |name: &str| {
            stamp.lines().filter(|line| !line.starts_with('#')).find_map(|line| line.strip_prefix(name)?.strip_prefix(' ')).map(str::trim).unwrap_or_else(|| panic!("the stamp names no {name}"))
        };
        let shader = include_str!("../ui_pq.hlsl").replace("\r\n", "\n");
        assert_eq!(sha256(shader.as_bytes()), recorded("ui_pq.hlsl"),
            "ui_pq.hlsl changed: regenerate ui_pq_fragment.spv with the command in its header and update ui_pq_fragment.spv.sha256");
        assert_eq!(sha256(FRAGMENT_SPIRV), recorded("ui_pq_fragment.spv"),
            "ui_pq_fragment.spv changed without its stamp: put its SHA-256 in ui_pq_fragment.spv.sha256");
    }

    /// The encodes are the present passes' own, so the overlay has the brightness it had through
    /// the back buffer: the matrix, the PQ curve and the gamma 2.2 line are theirs to the letter.
    #[test]
    fn the_encodes_are_the_present_passes_own() {
        let squeezed = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
        let ours = squeezed(include_str!("../ui_pq.hlsl"));
        let scanout = squeezed(include_str!("../scanout_present.hlsl"));
        for line in [
            "0.6274040, 0.3292820, 0.0433136, 0.0690970, 0.9195400, 0.0113612, 0.0163916, 0.0880132, 0.8955950);",
            "float3 y = pow(saturate(nits / 10000.0), m1); return pow((c1 + c2 * y) / (1.0 + c3 * y), m2);",
            "pow(saturate((nits - minNits) * invRange), 1.0 / 2.2)",
            "(orderedDither(",
            "static const float bayer[16] = { 0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5 };",
        ] {
            assert!(ours.contains(line), "ui_pq.hlsl lacks {line}");
            assert!(scanout.contains(line), "scanout_present.hlsl lacks {line}");
        }
    }
}
